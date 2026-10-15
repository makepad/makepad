#[path = "../../bindgen_xml.rs"]
mod xml;
mod registry;
mod types;
mod commands;
use registry::{rust_type, Registry};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn main() {
    if let Err(error) = run() {
        eprintln!("vulkan-codegen: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        return Err("usage: makepad-vulkan-codegen <vk.xml> <apis.txt> <output directory>".into());
    }
    let source = std::fs::read_to_string(&args[1]).map_err(|e| e.to_string())?;
    let r = Registry::parse(&source);
    let roots = std::fs::read_to_string(&args[2]).map_err(|e| e.to_string())?;
    let mut selected: BTreeSet<String> = BTreeSet::new();
    let mut constants: BTreeSet<String> = BTreeSet::new();
    let mut commands: BTreeMap<String, String> = BTreeMap::new();
    let mut wanted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in roots.lines() {
        let line = line.split('#').next().unwrap().trim();
        if line.is_empty() { continue; }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 2 { return Err(format!("invalid API root: {line}")); }
        match fields[0] {
            "type" => {
                if !r.types.contains_key(fields[1]) { return Err(format!("unknown type {}",fields[1])); }
                r.require_type(fields[1], &mut selected);
            }
            "const" => { constants.insert(fields[1].to_string()); }
            "value" => { wanted.entry(fields[1].into()).or_default().insert(fields[2].into()); }
            "entry" | "instance" | "device" => {
                let command = r.command(fields[1]);
                commands.insert(fields[1].into(), fields[0].into());
                for f in &command.fields { r.require_type(&f.base, &mut selected); }
                for (name,_) in &r.types {
                    if rust_type(name) == command.ret { r.require_type(name, &mut selected); }
                }
            }
            _ => return Err(format!("invalid API root kind: {line}")),
        }
    }
    for (name, values) in wanted.clone() {
        let mut canonical = name;
        while let Some(t) = r.types.get(&canonical) {
            if t.alias.is_empty() { break; }
            canonical = t.alias.clone();
        }
        wanted.entry(canonical).or_default().extend(values);
    }
    r.require_type("PFN_vkVoidFunction", &mut selected);
    r.require_type("VkBaseOutStructure", &mut selected);
    let vk = types::emit(&r,&selected,&constants,&wanted);
    let functions = commands::emit(&r,&commands);
    let mut manifest = format!("Vulkan registry header {}\n",r.revision);
    for name in &selected { manifest.push_str(&format!("type {name}\n")); }
    for name in commands.keys() { manifest.push_str(&format!("command {name}\n")); }
    let output = Path::new(&args[3]);
    std::fs::create_dir_all(output).map_err(|e|e.to_string())?;
    for (name,text) in [("vk.rs",vk),("commands.rs",functions)] {
        std::fs::write(output.join(name),text).map_err(|e|e.to_string())?;
    }
    print!("{manifest}");
    Ok(())
}
