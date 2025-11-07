use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use crate::makepad_shell::*;

fn read_package_name_from_cargo_toml(cwd: &Path) -> Result<String, String> {
    let cargo_toml_path = cwd.join("Cargo.toml");
    let cargo_str = std::fs::read_to_string(&cargo_toml_path)
        .map_err(|e| format!("Failed to read Cargo.toml: {}", e))?;
    let toml = makepad_toml_parser::parse_toml(&cargo_str)
        .map_err(|e| format!("Failed to parse Cargo.toml: {:?}", e))?;
    let package_name = toml.get("package.name")
        .ok_or_else(|| "package.name not found in Cargo.toml".to_string())?
        .clone()
        .into_str()
        .ok_or_else(|| "package.name is not a string in Cargo.toml".to_string())?;
    Ok(package_name)
}

fn looks_like_version(s: &str) -> bool {
    // Check if the string looks like a version number (contains dots and digits)
    // Simple heuristic: if it contains at least one dot and at least one digit
    s.contains('.') && s.chars().any(|c| c.is_ascii_digit())
}

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
                if path.is_dir(){
                    return Some((name.to_string(), Some(path.into())))
                }
            }
        }
        return Some((name.to_string(), None))
    }
    None
}

pub fn get_crate_dir(build_crate: &str) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().unwrap();
    if let Ok(output) = shell_env_cap(&[], &cwd, "cargo", &["pkgid", "-p", build_crate]) {
        #[cfg(target_os="windows")]
        {
            let output = output.strip_prefix("file:///").unwrap_or(&output);
            let output = output.strip_prefix("path+file:///").unwrap_or(output);
            return Ok(output.split('#').next().unwrap().into());
        }
        #[cfg(not(target_os="windows"))]
        {  
            let output = output.strip_prefix("file://").unwrap_or(&output);
            let output = output.strip_prefix("path+file://").unwrap_or(output);
            return Ok(output.split('#').next().unwrap().into());
        }
    } else {
        Err(format!("Failed to get crate dir for: {}", build_crate))
    }
}

pub fn get_crate_dep_dirs(build_crate: &str, build_dir:&Path, target:&str) -> HashMap<String, PathBuf> {
    let mut dependencies = HashMap::new();
    let cwd = std::env::current_dir().unwrap();
    let target = format!("--target={target}");
    if let Ok(cargo_tree_output) = shell_env_cap(&[], &cwd, "cargo", &["tree", "-p", build_crate, &target]) {
        for line in cargo_tree_output.lines().skip(1) {
            if let Some((name, path)) = extract_dependency_paths(line) {
                if let Some(path) = path{
                    dependencies.insert(name, path);
                }
                else { // check in the build dir for .path files, used to find the crate dir of a crates.io crate
                    let dir_file = build_dir.join(format!("{}.path", name));
                    if let Ok(path) = std::fs::read_to_string(&dir_file){
                        dependencies.insert(name, Path::new(path.trim()).into());
                    }
                }
            }
        }
    }
    dependencies
}

pub fn get_build_crate_from_args(args: &[String]) -> Result<String, String> {
    if args.is_empty() {
        // Auto-detect package from current directory
        let cwd = std::env::current_dir().map_err(|e| format!("Failed to get current directory: {}", e))?;
        if let Ok(output) = shell_env_cap(&[], &cwd, "cargo", &["pkgid"]) {
            // Extract package name from cargo pkgid output
            // Format is typically: path+file:///path/to/crate#package_name@version
            // But sometimes it's just: path+file:///path/to/crate#version
            if let Some(package_part) = output.split('#').nth(1) {
                let trimmed_part = package_part.trim();
                // Check if there's an @ symbol separating package name from version
                if trimmed_part.contains('@') {
                    // Format: package_name@version
                    if let Some(package_name) = trimmed_part.split('@').next() {
                        let package_name = package_name.trim();
                        // If it looks like a version, it means parsing went wrong
                        if !looks_like_version(package_name) {
                            return Ok(package_name.to_string());
                        }
                    }
                } else {
                    // No @ symbol - could be just version or just package name
                    // If it looks like a version, read from Cargo.toml instead
                    if looks_like_version(trimmed_part) {
                        return read_package_name_from_cargo_toml(&cwd);
                    }
                    // Otherwise, treat as package name
                    return Ok(trimmed_part.to_string());
                }
            }
        }
        // Fallback: try reading from Cargo.toml
        if let Ok(package_name) = read_package_name_from_cargo_toml(&cwd) {
            return Ok(package_name);
        }
        return Err("Not enough arguments to build and could not auto-detect package from current directory. Please specify with --example <name> or -p <package>".into());
    }
    if args[0] == "-p" {
        if args.len()<2 { 
            return Err("Not enough arguments to build".into());
        }
        Ok(args[1].clone())
    }
    else {
        // First arg might be a cargo flag like --example, --bin, etc.
        // Skip those and look for the actual package name
        // For now, if it doesn't start with --, treat it as a package name
        if args[0].starts_with("--") {
            // It's a cargo flag, we need to find the package differently
            // Try to auto-detect from current directory
            let cwd = std::env::current_dir().map_err(|e| format!("Failed to get current directory: {}", e))?;
            if let Ok(output) = shell_env_cap(&[], &cwd, "cargo", &["pkgid"]) {
                if let Some(package_part) = output.split('#').nth(1) {
                    let trimmed_part = package_part.trim();
                    // Check if there's an @ symbol separating package name from version
                    if trimmed_part.contains('@') {
                        // Format: package_name@version
                        if let Some(package_name) = trimmed_part.split('@').next() {
                            let package_name = package_name.trim();
                            // If it looks like a version, it means parsing went wrong
                            if !looks_like_version(package_name) {
                                return Ok(package_name.to_string());
                            }
                        }
                    } else {
                        // No @ symbol - could be just version or just package name
                        // If it looks like a version, read from Cargo.toml instead
                        if looks_like_version(trimmed_part) {
                            return read_package_name_from_cargo_toml(&cwd);
                        }
                        // Otherwise, treat as package name
                        return Ok(trimmed_part.to_string());
                    }
                }
            }
            // Fallback: try reading from Cargo.toml
            if let Ok(package_name) = read_package_name_from_cargo_toml(&cwd) {
                return Ok(package_name);
            }
            return Err(format!("Could not determine package name from cargo arguments. Please use -p <package> to specify the package explicitly."));
        }
        // It's a package name
        Ok(args[0].clone())
    }
}

pub fn get_profile_from_args(args: &[String]) -> String {
    for arg in args{
        if let Some(opt) = arg.strip_prefix("--profile=") {
            return opt.to_string();
        }
        if arg == "--release"{
            return "release".to_string()
        }
    }
    return "debug".to_string()
}

