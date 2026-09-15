use std::fs;
use std::path::{Path, PathBuf};

use makepad_toml_parser::Toml;
use makepad_digest::sha1;
use makepad_base64::base64;
use std::io::prelude::*;

fn main() {
    
    let ignore_list = [Path::new("./target").into(), Path::new("./tools").into(), Path::new("./local").into(),Path::new("./.git").into(), Path::new("./Cargo.toml").into()];
    
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
    
    
    let mut ver_crates = Vec::new();
    
    // iterate all found crates and build up version info/dep info
    for c in crates {
        let cargo_str = fs::read_to_string(&c.cargo).unwrap();
        
        let toml = match makepad_toml_parser::parse_toml(&cargo_str){
            Ok(toml)=>toml,
            Err(e)=>{
                println!("Error in toml {:?} {:?}", c.cargo, e);
                continue;
            }
        };

        let old_sha1 = if let Some(Toml::Str(ver, _)) = toml.get_path(&["package", "metadata", "makepad-auto-version"]) {
            ver.to_string()
        }
        else {
            continue;
        };
        let package_name = toml.get_path(&["package", "name"]).unwrap().clone().into_str().unwrap();
        let package_version = toml.get_path(&["package", "version"]).unwrap().clone().into_str().unwrap();

        // hash all the rs files
        let mut sha1 = sha1::Sha1::new();
        for file_path in c.rs_files {
            let file_str = fs::read_to_string(file_path).unwrap();
            sha1.update(file_str.as_bytes());
        }
        let data = sha1.finalise();
        let new_sha1 = String::from_utf8(base64::base64_encode(&data, &base64::BASE64_URL_SAFE)).unwrap();
        let mut deps = Vec::new();
        // every dependency declared with a version, in any dependency table
        for table in dependency_tables(&toml) {
            let segments: Vec<&str> = table.iter().map(String::as_str).collect();
            if let Some(Toml::Table(entries)) = toml.get_path(&segments) {
                for (dep, value) in entries {
                    if let Toml::Table(dep_table) = value {
                        if dep_table.contains_key("version") {
                            println!("GOT DEP {}", dep);
                            deps.push(dep.clone());
                        }
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
            let _: u64 = ver.parse().unwrap();
            
            let next_version = format!("1.0.0");//, version + 1);
            //let next_version = format!("0.4.0");
            
            patch_cargo(&c.cargo, &["package", "version"], &next_version, write);
            patch_cargo(&c.cargo, &["package", "metadata", "makepad-auto-version"], &c.new_sha1, write);
            // now lets version-up everyone elses dependency on this crate, in
            // whichever dependency table declares it with a version
            for o in &ver_crates {
                let other = fs::read_to_string(&o.cargo).unwrap();
                let Ok(other_toml) = makepad_toml_parser::parse_toml(&other) else {
                    continue;
                };
                for table in dependency_tables(&other_toml) {
                    let mut path = table.clone();
                    path.push(c.package_name.clone());
                    path.push("version".to_string());
                    let segments: Vec<&str> = path.iter().map(String::as_str).collect();
                    if other_toml.get_path(&segments).is_some() {
                        patch_cargo(&o.cargo, &segments, &next_version, write);
                    }
                }
            }
        }
    }

    println!("Done");
}

/// Every dependency table of a manifest as a key path: `dependencies`,
/// `dev-dependencies`, `build-dependencies` and each `target.<cfg>.<kind>`
/// table that is present.
fn dependency_tables(toml: &makepad_toml_parser::TomlDocument) -> Vec<Vec<String>> {
    let kinds = ["dependencies", "dev-dependencies", "build-dependencies"];
    let mut tables = Vec::new();
    for kind in kinds {
        if let Some(Toml::Table(_)) = toml.get_path(&[kind]) {
            tables.push(vec![kind.to_string()]);
        }
    }
    if let Some(Toml::Table(targets)) = toml.get_path(&["target"]) {
        for (target, value) in targets {
            if let Toml::Table(table) = value {
                for kind in kinds {
                    if let Some(Toml::Table(_)) = table.get(kind) {
                        tables.push(vec!["target".to_string(), target.clone(), kind.to_string()]);
                    }
                }
            }
        }
    }
    tables
}

fn patch_cargo(cargo: &Path, toml_path: &[&str], with: &str, write: bool) {
    let old_cargo = fs::read_to_string(cargo).unwrap();
    let toml = makepad_toml_parser::parse_toml(&old_cargo).unwrap();

    if let Some(Toml::Str(_, span)) = toml.get_path(toml_path) {
        // The span covers the quoted lexeme in bytes; replace the whole string.
        let mut new_cargo = String::with_capacity(old_cargo.len() + with.len());
        new_cargo.push_str(&old_cargo[..span.start]);
        new_cargo.push('"');
        new_cargo.push_str(with);
        new_cargo.push('"');
        new_cargo.push_str(&old_cargo[span.start + span.len..]);
        // lets write it back to disk
        if write {
            fs::File::create(cargo).unwrap().write_all(new_cargo.as_bytes()).unwrap();
            println!("Updating {:?} with {}", cargo, with);
        }
        else {
            println!("Would have updated {:?} with {}", cargo, with);
        }
    }
}
