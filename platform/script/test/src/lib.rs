//! Reusable text-only shader inventory for app startup registrations.
//! Call after registration, without constructing the app or starting its event loop.

use makepad_core_util::sha256::sha256_hex;
use makepad_script::{
    shader_backend::ShaderBackend,
    shader_compile::{compile_shader, ShaderCompileOptions},
    *,
};
use std::{collections::{BTreeMap, HashMap, VecDeque}, io::Write, path::Path};

/// Configure the existing deterministic clock before constructing Cx. Some
/// registration hooks read application time for tracing. Requesting remote
/// mode enables that clock; this harness never starts the bridge/event loop.
/// Run inventories in their own process (--exact shader_output_inventory).
pub fn prepare_shader_inventory() {
    std::env::set_var("MAKEPAD_REMOTE", "1");
    std::env::set_var("MAKEPAD_VIRTUAL_CLOCK", "1");
    std::env::set_var("MAKEPAD_SEED", "0");
}

fn object_paths(vm: &ScriptVm) -> HashMap<ScriptObject, String> {
    let mut paths = HashMap::new();
    let mut queue = VecDeque::from([(vm.heap().modules, "mod".to_string())]);
    while let Some((object, path)) = queue.pop_front() {
        if paths.contains_key(&object) { continue; }
        paths.insert(object, path.clone());
        if vm.heap().as_fn(object).is_some() { continue; }
        let mut children = Vec::new();
        for entry in vm.heap().map_entries(object) {
            if let (Some(key), Some(child)) = (entry.key.as_id(), entry.value.as_object()) {
                children.push((key.to_string(), child));
            }
        }
        children.sort_by(|a, b| a.0.cmp(&b.0));
        for (key, child) in children { queue.push_back((child, format!("{path}.{key}"))); }
        for (index, entry) in vm.heap().vec_ref(object).iter().enumerate() {
            if let Some(child) = entry.value.as_object() {
                queue.push_back((child, format!("{path}[{index}]")));
            }
        }
        if let Some(proto) = vm.heap().proto(object).as_object() {
            queue.push_back((proto, format!("{path}::<proto>")));
        }
    }
    paths
}

/// Writes output hashes and a separate, explicit failure inventory. Every
/// allocated draw object is included, even anonymous startup definitions and
/// type defaults not reachable through the public module tree. Object order is
/// allocation order; a per-type ordinal distinguishes inherited definitions.
/// SPIR-V words are hashed in little-endian order. No backend validator runs.
pub fn write_shader_inventory(vm: &mut ScriptVm, label: &str, directory: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let mut frontends = std::fs::File::create(directory.join(format!("{label}.frontends.tsv")))?;
    writeln!(frontends, "shader\tbackend\tast_functions\ttotal_functions")?;
    let mut hashes = BTreeMap::new();
    let mut failures = BTreeMap::new();
    let mut names = BTreeMap::<String, usize>::new();
    let paths = object_paths(vm);
    let mut manifest = std::fs::File::create(directory.join(format!("{label}.objects.tsv")))?;
    let objects: Vec<_> = vm.heap().alloced_object_ids().into_iter().filter(|obj| {
        [live_id!(vertex), live_id!(fragment)].into_iter()
            .all(|name| vm.object_method(*obj, name).as_object().is_some())
    }).collect();
    vm.capture_errors();
    for obj in &objects {
        let ty = vm.heap().object_type_name_in_chain(*obj)
            .map(|name| name.to_string()).unwrap_or_else(|| "anonymous".into());
        let ordinal = names.entry(ty.clone()).or_default();
        let name = format!("{label}/{ty}/{ordinal:04}");
        *ordinal += 1;
        writeln!(manifest, "{name}\t{}", paths.get(obj).map(String::as_str).unwrap_or("<startup/anonymous>"))?;
        for (backend_name, backend, vulkan, webgl) in [
            ("metal", ShaderBackend::Metal, false, false),
            ("hlsl", ShaderBackend::Hlsl, false, false),
            ("glsl", ShaderBackend::Glsl, false, false),
            ("webgl2", ShaderBackend::Glsl, false, true),
            ("wgsl", ShaderBackend::Wgsl, true, false),
            ("spirv", ShaderBackend::Glsl, true, false),
            ("rust", ShaderBackend::Rust, false, false),
        ] {
            let mut opts = ShaderCompileOptions::new(backend, false, vulkan, webgl, false);
            opts.pick = backend_name == "metal";
            let output = compile_shader(vm, *obj, &opts);
            writeln!(frontends, "{name}\t{backend_name}\t{}\t{}", output.ast_function_count(), output.function_count())?;
            if output.has_errors() {
                failures.insert(format!("{name}\t{backend_name}"), output.error_report().to_owned());
                continue;
            }
            let mut record = |variant: &str, bytes: &[u8]| {
                hashes.insert(format!("{name}\t{variant}"), sha256_hex(bytes));
            };
            match backend_name {
                "metal" => {
                    record("metal", output.combined().as_bytes());
                    record("metal-pick", output.pick_code().unwrap().as_bytes());
                }
                "hlsl" => record("hlsl", output.combined().as_bytes()),
                "glsl" | "webgl2" => {
                    let source = output.glsl().unwrap();
                    record(&format!("{backend_name}-vertex"), source.vertex().as_bytes());
                    record(&format!("{backend_name}-fragment"), source.fragment().as_bytes());
                }
                "wgsl" => {
                    record("wgsl", output.combined().as_bytes());
                    opts.xr = true;
                    let xr = compile_shader(vm, *obj, &opts);
                    if xr.has_errors() { failures.insert(format!("{name}\twgsl-xr"), xr.error_report().to_owned()); }
                    else { record("wgsl-xr", xr.combined().as_bytes()); }
                }
                "spirv" => {
                    let result = output.spirv().unwrap();
                    if let Some(error) = result.error() { failures.insert(format!("{name}\tspirv"), error.to_owned()); }
                    else {
                        for i in 0..2 {
                            let variant = result.source(i).unwrap();
                            for (stage, words) in [("vertex", variant.vertex()), ("fragment", variant.fragment())] {
                                let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
                                record(&format!("spirv-{}-{stage}", if i == 0 { "window" } else { "xr" }), &bytes);
                            }
                        }
                    }
                }
                "rust" => record("rust", output.rust().unwrap().source().as_bytes()),
                _ => unreachable!(),
            }
        }
    }
    vm.stop_capturing_errors();
    let mut file = std::fs::File::create(directory.join(format!("{label}.sha256.tsv")))?;
    for (key, digest) in &hashes { writeln!(file, "{key}\t{digest}")?; }
    let mut file = std::fs::File::create(directory.join(format!("{label}.failures.tsv")))?;
    for (key, error) in &failures { writeln!(file, "{key}\t{}", error.replace('\n', "\\n").replace('\t', "\\t"))?; }
    eprintln!("{label}: {} shader objects, {} output hashes, {} backend failures", objects.len(), hashes.len(), failures.len());
    assert!(!objects.is_empty(), "startup registration found no shaders");
    Ok(())
}

/// Opt-in headless test of the application's actual registration entry point.
/// Run with MAKEPAD_SHADER_INVENTORY_DIR set and --ignored --exact shader_output_inventory.
#[macro_export]
macro_rules! shader_inventory_test {
    ($label:literal, $register:expr) => {
        #[test]
        #[ignore = "writes the shader baseline; set MAKEPAD_SHADER_INVENTORY_DIR"]
        fn shader_output_inventory() {
            let directory = std::env::var_os("MAKEPAD_SHADER_INVENTORY_DIR")
                .expect("set MAKEPAD_SHADER_INVENTORY_DIR to the output directory");
            $crate::prepare_shader_inventory();
            let mut cx = Cx::new(Box::new(|_, _| {}));
            cx.with_vm(|vm| {
                vm.capture_errors();
                ($register)(vm);
                let errors = vm.take_errors();
                assert!(errors.is_empty(), "startup registration failed:\n{}", errors.join("\n"));
                $crate::write_shader_inventory(vm, $label, std::path::Path::new(&directory)).unwrap();
            });
        }
    };
}
