use std::fs;
use std::path::{Path, PathBuf};
use makepad_toml_parser;
use makepad_toml_parser::Toml;
use makepad_digest::sha1;
use makepad_base64::base64;
use std::io::prelude::*;

fn main() {
    
    let ignore_list = [Path::new("./target").into(), Path::new("./.git").into(), Path::new("./Cargo.toml").into()];
    
    let mut crate_stack = Vec::new();
    let mut crates = Vec::new();
    
    // Crate with rs files
    struct Crate {
        cargo: PathBuf,
        rs_files: Vec<PathBuf>,
    }
    
    fn recur_read(path: &Path, ignore_list: &[PathBuf], crates: &mut Vec<Crate>, crate_stack: &mut Vec<usize>) {
        let iter = fs::read_dir(path);
        let iter = iter.unwrap();
        let mut dirs = Vec::new();
        let mut is_crate = false;
        for iter in iter {
            let iter = iter.unwrap();
            if ignore_list.iter().any( | v | *v == iter.path()) {
                continue
            }
            let meta = iter.metadata().unwrap();
            if meta.is_dir() {
                dirs.push(iter.path());
            }
            if iter.file_name() == "Cargo.toml" {
                crates.push(Crate {
                    cargo: iter.path(),
                    rs_files: Vec::new(),
                });
                crate_stack.push(crates.len() - 1);
                is_crate = true;
            }
            if let Some(os_str) = iter.path().extension() {
                if let Some("rs") = os_str.to_str() {
                    crates[*crate_stack.last().unwrap()].rs_files.push(iter.path());
                }
            }
        }
        for dir in dirs {
            recur_read(&dir, ignore_list, crates, crate_stack)
        }
        if is_crate {
            crates[*crate_stack.last().unwrap()].rs_files.sort();
            crate_stack.pop();
        }
    }
    // scan our filetree and build up a list of crates in our monorepo
    recur_read(Path::new("./"), &ignore_list, &mut crates, &mut crate_stack);
    
    // versioned crate
    struct VerCrate {
        package_name: String,
        package_version: String,
        cargo: PathBuf,
        deps: Vec<String>,
        old_sha1: String,
        new_sha1: String,
    }
    
    let target_deps = [
        "dependencies.",
        "target.wasm32-unknown-unknown.dependencies.",
        "target.aarch64-apple-darwin.dependencies.",
        "target.x86_64-unknown-linux-gnu.dependencies.",
        "target.armv7-unknown-linux-gnueabihf.dependencies.",
        "target.x86_64-pc-windows-gnu.dependencies.",
        "target.x86_64-pc-windows-msvc.dependencies."
    ];
    
    let mut ver_crates = Vec::new();
    
    // iterate all found crates and build up version info/dep info
    for c in crates {
        let cargo_str = fs::read_to_string(&c.cargo).unwrap();
        let toml = makepad_toml_parser::parse_toml(&cargo_str).unwrap();

        let old_sha1 = if let Some(Toml::Str(ver, _)) = toml.get("package.metadata.makepad-auto-version") {
            ver.to_string()
        }
        else {
            continue;
        };
        let package_name = toml.get("package.name").unwrap().clone().into_str().unwrap();
        let package_version = toml.get("package.version").unwrap().clone().into_str().unwrap();

        // hash all the rs files
        let mut sha1 = sha1::Sha1::new();
        for file_path in c.rs_files {
            let file_str = fs::read_to_string(file_path).unwrap();
            sha1.update(file_str.as_bytes());
        }
        let data = sha1.finalise();
        let new_sha1 = String::from_utf8(base64::base64_encode(&data, &base64::BASE64_URL_SAFE)).unwrap();
        let mut deps = Vec::new();
        // scan our toml file for all dependencies
        for key in toml.keys() {
            for pref in target_deps {
                if let Some(pref) = key.strip_prefix(pref) {
                    if let Some(dep) = pref.strip_suffix(".version") {
                        deps.push(dep.to_string());
                    }
                }
            }
        }
        ver_crates.push(VerCrate {
            cargo: c.cargo.clone(),
            package_name,
            package_version,
            deps,
            old_sha1,
            new_sha1,
        });
    }

    let args: Vec<String> = std::env::args().collect();
    let write = if args.len() >= 2 && args[1] == "-u" {
        println!("Updating auto versions");
        true
    }else {
        println!("Scanning auto version -- pass -u as argument to update");
        false
    };
        
    // scan if any of a crates dependencies has a hash change, ifso, up the version
    for c in &ver_crates {
        fn any_dep_changed(c: &VerCrate, ver_crates: &[VerCrate]) -> bool {
            //return true;
            if c.old_sha1 != c.new_sha1 {
                return true
            }
            for dep in &c.deps {
                if let Some(c) = ver_crates.iter().find( | v | &v.package_name == dep) {
                    if any_dep_changed(c, ver_crates) {
                        return true
                    }
                }
            }
            false
        }
        if any_dep_changed(c, &ver_crates) {
            let ver = c.package_version.strip_prefix("0.").unwrap().strip_suffix(".0").unwrap();
            let version: u64 = ver.parse().unwrap();
            
            let next_version = format!("0.{}.0", version + 1);
            //let next_version = format!("0.3.0");
            
            patch_cargo(&c.cargo, "package.version", &next_version, write);
            patch_cargo(&c.cargo, "package.metadata.makepad-auto-version", &c.new_sha1, write);
            // now lets version-up everyone elses dependency on this crate
            for pref in target_deps {
                let dep_version = format!("{}{}.version", pref, c.package_name);
                for o in &ver_crates {
                    patch_cargo(&o.cargo, &dep_version, &next_version, write);
                }
            }
        }
    }

    println!("Done");
}

fn patch_cargo(cargo: &Path, toml_path: &str, with: &str, write: bool) {
    let old_cargo = fs::read_to_string(&cargo).unwrap();
    let toml = makepad_toml_parser::parse_toml(&old_cargo).unwrap();
    
    if let Some(Toml::Str(_, span)) = toml.get(toml_path) {
        let mut new_cargo = String::new();
        for (i, c) in old_cargo.chars().enumerate() {
            if i == span.start {
                for c in with.chars() {
                    new_cargo.push(c);
                }
            }
            if i < span.start || i >= span.start + span.len - 2 {
                new_cargo.push(c);
            }
        }
        // lets write it back to disk
        if write {
            fs::File::create(&cargo).unwrap().write_all(new_cargo.as_bytes()).unwrap();
            println!("Updating {:?} with {}", cargo, with);
        }
        else {
            println!("Would have updated {:?} with {}", cargo, with);
        }
    }
}

