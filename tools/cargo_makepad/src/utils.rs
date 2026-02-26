use crate::makepad_shell::*;
use makepad_toml_parser::{Toml, parse_toml};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub fn extract_dependency_paths(line: &str) -> Option<(String, Option<PathBuf>)> {
    let dependency_output_start = line.find(|c: char| c.is_alphanumeric())?;
    let dependency_output = &line[dependency_output_start..];

    let mut tokens = dependency_output.split(' ');
    if let Some(name) = tokens.next() {
        for token in tokens.collect::<Vec<&str>>() {
            if token == "(*)" || token == "(proc-macro)" {
                continue;
            }
            if token.starts_with('(') {
                let path = token[1..token.len() - 1].to_owned();
                let path = Path::new(&path);
                if path.is_dir() {
                    return Some((name.to_string(), Some(path.into())));
                }
            }
        }
        return Some((name.to_string(), None));
    }
    None
}

pub fn get_crate_dir(build_crate: &str) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().unwrap();
    if let Ok(output) = shell_env_cap(&[], &cwd, "cargo", &["pkgid", "-p", build_crate]) {
        #[cfg(target_os = "windows")]
        {
            let output = output.strip_prefix("file:///").unwrap_or(&output);
            let output = output.strip_prefix("path+file:///").unwrap_or(output);
            return Ok(output.split('#').next().unwrap().into());
        }
        #[cfg(not(target_os = "windows"))]
        {
            let output = output.strip_prefix("file://").unwrap_or(&output);
            let output = output.strip_prefix("path+file://").unwrap_or(output);
            return Ok(output.split('#').next().unwrap().into());
        }
    } else {
        Err(format!("Failed to get crate dir for: {}", build_crate))
    }
}

pub fn get_crate_dep_dirs(
    build_crate: &str,
    build_dir: &Path,
    target: &str,
) -> HashMap<String, PathBuf> {
    let mut dependencies = HashMap::new();
    let cwd = std::env::current_dir().unwrap();
    let target = format!("--target={target}");
    if let Ok(cargo_tree_output) = shell_env_cap(
        &[],
        &cwd,
        "cargo",
        &["tree", "--color", "never", "-p", build_crate, &target],
    ) {
        for line in cargo_tree_output.lines().skip(1) {
            if let Some((name, path)) = extract_dependency_paths(line) {
                if let Some(path) = path {
                    dependencies.insert(name, path);
                } else {
                    // check in the build dir for .path files, used to find the crate dir of a crates.io crate
                    let dir_file = build_dir.join(format!("{}.path", name));
                    if let Ok(path) = std::fs::read_to_string(&dir_file) {
                        dependencies.insert(name, Path::new(&path).into());
                    }
                }
            }
        }
    }
    dependencies
}

pub fn get_package_binary_name(build_crate: &str) -> Option<String> {
    let crate_dir = get_crate_dir(build_crate).ok()?;
    let cargo_toml = std::fs::read_to_string(crate_dir.join("Cargo.toml")).ok()?;

    let mut in_bin = false;
    for raw in cargo_toml.lines() {
        let line = raw.trim();
        if line.starts_with("[[bin]]") {
            in_bin = true;
            continue;
        }
        if line.starts_with('[') {
            in_bin = false;
        }
        if in_bin && line.starts_with("name") {
            if let Some(eq) = line.find('=') {
                let value = line[eq + 1..].trim().trim_matches('"').to_string();
                if !value.is_empty() {
                    return Some(value);
                }
            }
        }
    }

    let toml = parse_toml(&cargo_toml).ok()?;
    if let Some(Toml::Str(pkg_name, _)) = toml.get("package.name") {
        return Some(pkg_name.clone());
    }
    None
}

pub fn get_build_crate_from_args(args: &[String]) -> Result<&str, String> {
    if args.is_empty() {
        return Err("Not enough arguments to determine crate. Pass -p <crate> or --package <crate>.".into());
    }

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-p" || arg == "--package" {
            if i + 1 >= args.len() {
                return Err("Missing crate name after -p/--package".into());
            }
            return Ok(&args[i + 1]);
        }
        if let Some(pkg) = arg.strip_prefix("--package=") {
            if pkg.is_empty() {
                return Err("Missing crate name in --package=<crate>".into());
            }
            return Ok(pkg);
        }
        i += 1;
    }

    if let Some(first_positional) = args.iter().find(|a| !a.starts_with('-')) {
        return Ok(first_positional);
    }

    Err("No build crate specified. Pass -p <crate> or --package <crate>.".into())
}

pub fn get_target_from_args(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--target" {
            if i + 1 < args.len() {
                return Some(args[i + 1].clone());
            }
            return None;
        }
        if let Some(t) = arg.strip_prefix("--target=") {
            if !t.is_empty() {
                return Some(t.to_string());
            }
            return None;
        }
        i += 1;
    }
    None
}

pub fn get_profile_from_args(args: &[String]) -> String {
    for arg in args {
        if let Some(opt) = arg.strip_prefix("--profile=") {
            return opt.to_string();
        }
        if arg == "--release" {
            return "release".to_string();
        }
    }
    return "debug".to_string();
}

pub const APP_ICON_COUNT: usize = 7;
pub const APP_ICON_IDX_512: usize = 4;
pub const APP_ICON_IDX_1024: usize = 5;
pub const APP_ICON_IDX_ICO: usize = 6;

pub type AppIconEnv = [String; APP_ICON_COUNT];

pub const APP_ICON_ENV_VARS: [&str; APP_ICON_COUNT] = [
    "MAKEPAD_APP_ICON_32",
    "MAKEPAD_APP_ICON_64",
    "MAKEPAD_APP_ICON_128",
    "MAKEPAD_APP_ICON_256",
    "MAKEPAD_APP_ICON_512",
    "MAKEPAD_APP_ICON_1024",
    "MAKEPAD_APP_ICON_ICO",
];

pub fn resolve_app_icon_env(build_crate: &str) -> Result<Option<AppIconEnv>, String> {
    let resources_dir = get_crate_dir(build_crate)?.join("resources");
    let required_paths = [
        resources_dir.join("icon_32.png"),
        resources_dir.join("icon_64.png"),
        resources_dir.join("icon_128.png"),
        resources_dir.join("icon.ico"),
    ];

    if !required_paths.iter().all(|p| p.is_file()) {
        for path in &required_paths {
            if !path.is_file() {
                eprintln!(
                    "warning: missing {}. Add this file to include a custom app icon.",
                    path.display()
                );
            }
        }
        return Ok(None);
    }

    let optional = |name: &str, fallback: &Path| {
        let path = resources_dir.join(name);
        if path.is_file() {
            path
        } else {
            fallback.to_path_buf()
        }
    };

    let icon_256 = optional("icon_256.png", &required_paths[2]);
    let icon_512 = optional("icon_512.png", &icon_256);
    let icon_1024 = optional("icon_1024.png", &icon_512);

    Ok(Some([
        required_paths[0].to_string_lossy().to_string(),
        required_paths[1].to_string_lossy().to_string(),
        required_paths[2].to_string_lossy().to_string(),
        icon_256.to_string_lossy().to_string(),
        icon_512.to_string_lossy().to_string(),
        icon_1024.to_string_lossy().to_string(),
        required_paths[3].to_string_lossy().to_string(),
    ]))
}
