use std::fs;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use makepad_toml_parser;
use makepad_toml_parser::Toml;
use makepad_digest::sha1;
use makepad_base64::base64;

fn main() {

    let ignore_list = [Path::new("./target").into(), Path::new("./.git").into(), Path::new("./Cargo.toml").into()];
    
    let mut crate_stack = Vec::new();
    let mut crates = Vec::new();
    
    struct Crate{
        cargo:PathBuf,
        rs_files:Vec<PathBuf>,
    }
    
    fn recur_read(path:&Path, ignore_list:&[PathBuf], crates:&mut Vec<Crate>, crate_stack:&mut Vec<usize>){
        let iter = fs::read_dir(path);
        let iter = iter.unwrap();
        let mut dirs = Vec::new();
        let mut is_crate = false;
        for iter in iter{
            let iter = iter.unwrap();
            if ignore_list.iter().any(|v| *v == iter.path()){
                continue
            }
            let meta = iter.metadata().unwrap();
            if meta.is_dir(){
                dirs.push(iter.path());
            }
            // ok so. if we find a Cargo file here we create a new module stack item.
            if iter.file_name() == "Cargo.toml"{
                crates.push(Crate{
                    cargo: iter.path(),
                    rs_files: Vec::new(),
                });
                crate_stack.push(crates.len() - 1);
                is_crate = true;
            }
            if let Some(os_str) = iter.path().extension(){
                if let Some("rs") = os_str.to_str(){
                    crates[*crate_stack.last().unwrap()].rs_files.push(iter.path());
                }
            }
        }
        for dir in dirs{
            recur_read(&dir, ignore_list, crates, crate_stack)
        }
        if is_crate{
            crates[*crate_stack.last().unwrap()].rs_files.sort();
            crate_stack.pop();
        }
    }
    recur_read(Path::new("./"), &ignore_list, &mut crates, &mut crate_stack);
    
    #[derive(Debug)]
    struct VerCrate{
        package_name: String,
        package_version: String,
        next_version: RefCell<Option<String>>,
        cargo:PathBuf,
        deps:Vec<String>,
        old_sha1:String,
        new_sha1:String,
    }
    
    let mut ver_crates = Vec::new();
    
    for c in crates{
        // ok now what. now we need to build a dependency tree.
        //println!("GOT CRATE {:?}", c.cargo);
        let cargo_str = fs::read_to_string(&c.cargo).unwrap();
        let toml = makepad_toml_parser::parse_toml(&cargo_str).unwrap();
        // alright so we have our toml
        let old_sha1 = if let Some(Toml::Str(ver,_)) = toml.get("package.metadata.makepad-auto-version"){
            ver.to_string()
        }
        else{
            continue;
        }; 
        let package_name = toml.get("package.name").unwrap().clone().into_str().unwrap();
        let package_version = toml.get("package.version").unwrap().clone().into_str().unwrap();
        // how do we do this
        // sha1 all files
        let mut sha1 = sha1::Sha1::new();
        for file_path in c.rs_files{
            let file_str = fs::read_to_string(file_path).unwrap();
            sha1.update(file_str.as_bytes());
        } 
        let data = sha1.finalise();
        let new_sha1 =  String::from_utf8(base64::base64_encode(&data, &base64::BASE64_URL_SAFE)).unwrap();
        let mut deps = Vec::new();
        for key in toml.keys(){
            if let Some(pref) = key.strip_prefix("dependencies."){
                if let Some(dep) = pref.strip_suffix(".version"){
                    deps.push(dep.to_string());
                }
            }
        }
        // lets find all our dependencies
        ver_crates.push(VerCrate{
            cargo: c.cargo.clone(),
            package_name,
            package_version,
            next_version: RefCell::new(None),
            deps,
            old_sha1,
            new_sha1,
        });
    }
    
    for c in &ver_crates{
        fn any_dep_changed(c:&VerCrate, ver_crates:&[VerCrate])->bool{
            if c.old_sha1 != c.new_sha1{
                return true
            }
            for dep in &c.deps{
                if let Some(c) = ver_crates.iter().find(|v| &v.package_name == dep){
                    if any_dep_changed(c, ver_crates){
                        return true
                    }
                }
            }
            false
        }
        if any_dep_changed(c, &ver_crates){
            let ver = c.package_version.strip_prefix("0.").unwrap().strip_suffix(".0").unwrap();
            let version:u64 = ver.parse().unwrap();
            *c.next_version.borrow_mut() = Some(format!("0.{}.0", version + 1));
        }
    }
    
    for c in &ver_crates{
        // ok lets patch up our Cargo.tomls all our 'next versions' should be set
        if let Some(next_version) = c.next_version.borrow().clone(){
            // alright so. we have a version update here.
            // ok so first we patch our own version
            patch_toml(&c.cargo, "package.version", &next_version);
            // ok now lets patch everyone elses dependency on this one
            let dep_version = format!("dependencies.{}.version",c.package_name);
            for o in &ver_crates{
                patch_toml(&o.cargo, &dep_version, &next_version);
            }
        }
    }
}

fn patch_toml(cargo:&Path, toml_path:&str, with:&str)->Option<String>{
    let cargo_str = fs::read_to_string(&cargo).unwrap();
    let toml = makepad_toml_parser::parse_toml(&cargo_str).unwrap();
    
    if let Some(Toml::Str(_,span)) = toml.get(toml_path){
        println!("HERE!");
        let mut ret = String::new();
        let mut inserted = false;
        for (i, c) in cargo_str.chars().enumerate() {
            if i == span.start {
                inserted = true;
                for c in with.chars() {
                    ret.push(c);
                }
            }
            if i < span.start || i >= span.start+span.len - 2 {
                ret.push(c);
            }
        }
        if !inserted { // end of string or empty string
            for c in with.chars() {
                ret.push(c);
            }
        }
        return Some(ret);
    }
    None
}

