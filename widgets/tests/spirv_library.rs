//! Every draw shader of the widget library (the `mod.draw` and `mod.widgets`
//! trees with the default families) compiles to SPIR-V through the path the
//! Vulkan backend takes (the shader IR and its SPIR-V printer),
//! window and XR variants, and each stage passes `spirv-val` when it is
//! installed. The GLSL of the same shaders passes `glslangValidator` when it
//! is installed, their HLSL `D3DCompile` on Windows, their MSL (and its pick
//! variant) the Metal device's compile on macOS (what the HAL ships: no offline
//! toolchain, no window; no Metal device is a failure), and their WGSL a
//! structural check. With `MAKEPAD_TEST_METAL_TOOLCHAIN=1` the MSL also goes
//! through the offline compiler (`xcrun metal`, Xcode's optional Metal
//! Toolchain); never on the CI path.

use makepad_widgets::makepad_script::{
    shader_backend::ShaderBackend,
    shader_ir_draw::{compile_draw_shader_ir_with, draw_spirv, IrCompileOptions},
    shader_output::ShaderOutput,
    shader_ir_glsl::glsl_draw_sources,
    shader_ir_hlsl::hlsl_draw_source,
    shader_ir_metal::metal_draw_source,
    shader_ir_wgsl::draw_wgsl,
    shader_ir_spirv::spirv_val,
    value::ScriptObject,
};
use makepad_widgets::*;
use std::collections::HashSet;

fn is_draw_shader(vm: &ScriptVm, obj: ScriptObject) -> bool {
    let has = |name: LiveId| vm.object_method(obj, name).as_object().is_some();
    has(live_id!(vertex)) && has(live_id!(fragment))
}

fn walk(vm: &ScriptVm, obj: ScriptObject, visited: &mut HashSet<ScriptObject>, found: &mut Vec<ScriptObject>) {
    if !visited.insert(obj) {
        return;
    }
    if is_draw_shader(vm, obj) {
        found.push(obj);
    }
    let mut children = Vec::new();
    for v in vm.heap().vec_ref(obj) {
        if let Some(o) = v.value.as_object() {
            children.push(o);
        }
    }
    for v in vm.heap().map_entries(obj) {
        if let Some(o) = v.value.as_object() {
            children.push(o);
        }
    }
    for child in children {
        walk(vm, child, visited, found);
    }
}

/// The compile the Vulkan renderer does, or None when the shader does not
/// compile at all (an abstract base without its pixel function, ...).
fn vulkan_compile(vm: &mut ScriptVm, io_self: ScriptObject) -> Option<ShaderOutput> {
    let has = |vm: &ScriptVm, n: LiveId| vm.object_method(io_self, n).as_object().is_some();
    if !has(vm, live_id!(vertex)) || !has(vm, live_id!(fragment)) {
        return None;
    }
    let opts = IrCompileOptions { use_vulkan: true, backend: ShaderBackend::Glsl, ..Default::default() };
    let output = compile_draw_shader_ir_with(vm, io_self, &opts);
    if output.has_errors {
        return None;
    }
    Some(output)
}

#[test]
fn every_library_draw_shader_compiles_to_valid_spirv() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_widgets::script_mod(vm);
        vm.capture_errors();
        let roots = [script_eval!(vm, { mod.draw }), script_eval!(vm, { mod.widgets })];
        let mut visited = HashSet::new();
        let mut found = Vec::new();
        for root in roots {
            if let Some(obj) = root.as_object() {
                walk(vm, obj, &mut visited, &mut found);
            }
        }
        let mut modules = HashSet::new();
        let mut glsl_seen = HashSet::new();
        let mut hlsl_seen = HashSet::new();
        let mut metal_seen = HashSet::new();
        let (mut skipped, mut lowering_failed) = (0, 0);
        let mut failures = Vec::new();
        // The script VM generates every source on this thread; the compiles and validators
        // (each distinct source once) run afterwards on all cores.
        let mut checks: Vec<(String, Check)> = Vec::new();
        for obj in &found {
            let Some(vk) = vulkan_compile(vm, *obj) else {
                skipped += 1;
                continue;
            };
            // GLSL ES 3.00 as the GL backend links it.
            let opts = IrCompileOptions { backend: ShaderBackend::Glsl, ..Default::default() };
            let mut gl = compile_draw_shader_ir_with(vm, *obj, &opts);
            if gl.has_errors {
                failures.push(format!("{:?}: GL IR: {}", vm.heap().object_type_name_in_chain(*obj), gl.error_report()));
            } else {
                let src = glsl_draw_sources(&mut gl, vm);
                for (stage, text) in [("vert", &src.vertex), ("frag", &src.fragment)] {
                    if !glsl_seen.insert(text.clone()) {
                        continue;
                    }
                    checks.push((format!("{:?}: glslang {stage}", vm.heap().object_type_name_in_chain(*obj)), Check::Glsl(stage, text.clone())));
                }
            }
            // HLSL as D3D11 compiles it (D3DCompile, on Windows).
            let opts = IrCompileOptions { backend: ShaderBackend::Hlsl, ..Default::default() };
            let hl = compile_draw_shader_ir_with(vm, *obj, &opts);
            if hl.has_errors {
                failures.push(format!("{:?}: HLSL IR: {}", vm.heap().object_type_name_in_chain(*obj), hl.error_report()));
            } else {
                let src = hlsl_draw_source(&hl, vm);
                if hlsl_seen.insert(src.clone()) {
                    checks.push((format!("{:?}: D3DCompile", vm.heap().object_type_name_in_chain(*obj)), Check::Hlsl(src)));
                }
            }
            // MSL as the Metal backend builds it, and its pick variant
            // (compiled by the Metal device, on macOS).
            let opts = IrCompileOptions { backend: ShaderBackend::Metal, ..Default::default() };
            let mt = compile_draw_shader_ir_with(vm, *obj, &opts);
            if mt.has_errors {
                failures.push(format!("{:?}: Metal IR: {}", vm.heap().object_type_name_in_chain(*obj), mt.error_report()));
            } else {
                for pick in [false, true] {
                    let src = metal_draw_source(&mt, vm, pick);
                    if !metal_seen.insert(src.clone()) {
                        continue;
                    }
                    checks.push((format!("{:?}: metal (pick {pick})", vm.heap().object_type_name_in_chain(*obj)), Check::Metal(src)));
                }
            }
            // The WGSL of the same IR (structure only: no WGSL validator).
            for xr in [false, true] {
                match draw_wgsl(&vk, vm, xr) {
                    Ok((wgsl, _)) => {
                        if let Some(e) = wgsl_structure_error(&wgsl) {
                            failures.push(format!("{:?}: WGSL {e}\n{wgsl}", vm.heap().object_type_name_in_chain(*obj)));
                        }
                    }
                    Err(e) => failures.push(format!("{:?}: WGSL: {e}", vm.heap().object_type_name_in_chain(*obj))),
                }
            }
            match draw_spirv(&vk, vm) {
                Err(e) => {
                    lowering_failed += 1;
                    failures.push(format!("{:?}: {e}", vm.heap().object_type_name_in_chain(*obj)));
                }
                Ok(variants) => {
                    for v in variants {
                        for words in [v.vertex, v.fragment] {
                            if !modules.insert(words.clone()) {
                                continue;
                            }
                            checks.push((format!("{:?}: spirv-val", vm.heap().object_type_name_in_chain(*obj)), Check::Spirv(words)));
                        }
                    }
                }
            }
        }
        vm.stop_capturing_errors();
        let [validated, glsl_validated, hlsl_validated, metal_validated] = run_checks(&checks, &mut failures);
        eprintln!(
            "{} draw shader objects, {} distinct stages, {} passed spirv-val, {} did not compile, {} failed IR/SPIR-V",
            found.len(),
            modules.len(),
            validated,
            skipped,
            lowering_failed
        );
        eprintln!("{} GLSL stages passed glslangValidator", glsl_validated);
        eprintln!("{} HLSL sources passed D3DCompile (vs_5_0 + ps_5_0)", hlsl_validated);
        eprintln!("{} Metal sources compiled on the Metal device", metal_validated);
        assert!(modules.len() > 50, "the walk found only {} modules", modules.len());
        assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    });
}

/// One compile or validation of a distinct generated source.
enum Check {
    Glsl(&'static str, String),
    Hlsl(String),
    Metal(String),
    Spirv(Vec<u32>),
}

/// Runs every check on all cores (each is a separate compile or tool process; the Metal
/// device compiles are thread-safe) and returns how many passed per kind: SPIR-V, GLSL,
/// HLSL, Metal. Failures keep the order the checks were made in.
fn run_checks(checks: &[(String, Check)], failures: &mut Vec<String>) -> [usize; 4] {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    let next = AtomicUsize::new(0);
    let passed: [AtomicUsize; 4] = Default::default();
    let failed = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some((label, check)) = checks.get(i) else { break };
                let (kind, result) = match check {
                    Check::Spirv(words) => (0, spirv_val(words)),
                    Check::Glsl(stage, text) => (1, glslang(stage, text)),
                    Check::Hlsl(src) => (2, d3d_compile(src).map(|r| r.map_err(|e| format!("{e}\n{src}")))),
                    Check::Metal(src) => {
                        if let Some(Err(e)) = metal_toolchain_compile(src) {
                            failed.lock().unwrap().push((i, format!("{label} (xcrun metal): {e}\n{src}")));
                        }
                        (3, metal_compile(src).map(|r| r.map_err(|e| format!("{e}\n{src}"))))
                    }
                };
                match result {
                    Some(Ok(())) => {
                        passed[kind].fetch_add(1, Ordering::Relaxed);
                    }
                    Some(Err(e)) => failed.lock().unwrap().push((i, format!("{label}: {e}"))),
                    None => {}
                }
            });
        }
    });
    let mut failed = failed.into_inner().unwrap();
    failed.sort_by_key(|(i, _)| *i);
    failures.extend(failed.into_iter().map(|(_, f)| f));
    passed.map(|p| p.into_inner())
}

/// What can be checked of WGSL text without a WGSL compiler: both entry
/// points, balanced brackets, and no placeholder the printer writes for a
/// missing value.
fn wgsl_structure_error(wgsl: &str) -> Option<String> {
    if !wgsl.contains("@vertex\nfn vertex_main(") || !wgsl.contains("@fragment\nfn fragment_main(") {
        return Some("lacks an entry point".to_string());
    }
    let (mut paren, mut brace, mut bracket) = (0i32, 0i32, 0i32);
    for c in wgsl.chars() {
        match c {
            '(' => paren += 1,
            ')' => paren -= 1,
            '{' => brace += 1,
            '}' => brace -= 1,
            '[' => bracket += 1,
            ']' => bracket -= 1,
            _ => {}
        }
        if paren < 0 || brace < 0 || bracket < 0 {
            return Some("has unbalanced brackets".to_string());
        }
    }
    if paren != 0 || brace != 0 || bracket != 0 {
        return Some("has unbalanced brackets".to_string());
    }
    if wgsl.contains(": void") || wgsl.contains("void(") {
        return Some("names the void type".to_string());
    }
    None
}

/// `glslangValidator` on a GL stage with the preamble the GL backend
/// prepends (window variant); None when the tool is not installed.
fn glslang(stage: &str, body: &str) -> Option<Result<(), String>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let helpers = "vec4 depth_clip(vec4 w, vec4 c, float clip);\n\
        vec4 sample2d(sampler2D sampler, vec2 pos){return texture(sampler, vec2(pos.x, pos.y));}\n\
        vec4 sample2d_lod(sampler2D sampler, vec2 pos, float lod){return textureLod(sampler, vec2(pos.x, pos.y), lod);}\n\
        vec4 samplecube(samplerCube sampler, vec3 dir){return texture(sampler, dir);}\n\
        vec4 samplecube_lod(samplerCube sampler, vec3 dir, float lod){return textureLod(sampler, dir, lod);}\n\
        #define sample2dOES(sampler, pos) (texture((sampler), vec2((pos).x, (pos).y)))\n\
        #define samplerExternalOES sampler2D\n";
    let tail = if stage == "frag" { "vec4 depth_clip(vec4 w, vec4 c, float clip){return c;}\n" } else { "" };
    let src = format!(
        "#version 300 es\n#define VIEW_ID 0\nprecision highp float;\nprecision highp int;\nprecision highp sampler2D;\nprecision highp sampler2DShadow;\nprecision highp sampler2DArray;\nprecision highp samplerCube;\n{helpers}{}{tail}",
        body.replace("[int(VIEW_ID)]", "[0]")
    );
    let path = std::env::temp_dir().join(format!("makepad-glsl-{}-{}.{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed), stage));
    std::fs::write(&path, &src).ok()?;
    let out = std::process::Command::new("glslangValidator").arg(&path).output();
    let _ = std::fs::remove_file(&path);
    match out {
        Err(_) => None,
        Ok(o) if o.status.success() => Some(Ok(())),
        Ok(o) => Some(Err(format!("{}\n{}", String::from_utf8_lossy(&o.stdout), src))),
    }
}

/// D3DCompile of both entry points with the flags D3D11 compiles with;
/// None off Windows.
#[cfg(target_os = "windows")]
fn d3d_compile(hlsl: &str) -> Option<Result<(), String>> {
    use makepad_widgets::makepad_platform::windows::{core::PCSTR, Win32::Graphics::Direct3D::Fxc::D3DCompile};
    const FLAGS: u32 = (1 << 2) | (1 << 12);
    for (target, entry) in [("vs_5_0\0", "vertex_main\0"), ("ps_5_0\0", "pixel_main\0")] {
        unsafe {
            let mut blob = None;
            let mut errors = None;
            let ok = D3DCompile(
                hlsl.as_ptr() as *const _,
                hlsl.len(),
                Some(PCSTR("makepad_shader\0".as_ptr())),
                None,
                None,
                Some(PCSTR(entry.as_ptr())),
                PCSTR(target.as_ptr()),
                FLAGS,
                0,
                &mut blob,
                Some(&mut errors),
            )
            .is_ok();
            if !ok {
                let msg = match errors {
                    Some(e) => String::from_utf8_lossy(std::slice::from_raw_parts(e.GetBufferPointer() as *const u8, e.GetBufferSize())).into_owned(),
                    None => "D3DCompile failed".to_string(),
                };
                return Some(Err(format!("{}: {}", entry.trim_end_matches('\0'), msg)));
            }
        }
    }
    Some(Ok(()))
}

#[cfg(not(target_os = "windows"))]
fn d3d_compile(_hlsl: &str) -> Option<Result<(), String>> {
    None
}

/// The Metal device's compile of a library, as the HAL compiles a pipeline
/// (`newLibraryWithSource`, its options, both entry points); None off macOS.
#[cfg(target_os = "macos")]
fn metal_compile(src: &str) -> Option<Result<(), String>> {
    Some(makepad_widgets::makepad_platform::makepad_hal::backend::apple::metal::compile_msl(src))
}

#[cfg(not(target_os = "macos"))]
fn metal_compile(_src: &str) -> Option<Result<(), String>> {
    None
}

/// Opt-in (`MAKEPAD_TEST_METAL_TOOLCHAIN=1`, macOS): `xcrun metal -c` of a
/// library with the offline compiler of Xcode's optional Metal Toolchain.
/// None when not asked for.
fn metal_toolchain_compile(src: &str) -> Option<Result<(), String>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    if !cfg!(target_os = "macos") || std::env::var("MAKEPAD_TEST_METAL_TOOLCHAIN").as_deref() != Ok("1") {
        return None;
    }
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("makepad-msl-{}-{}.metal", std::process::id(), n));
    let air = std::env::temp_dir().join(format!("makepad-msl-{}-{}.air", std::process::id(), n));
    if let Err(e) = std::fs::write(&path, src) {
        return Some(Err(format!("writing {}: {e}", path.display())));
    }
    let out = std::process::Command::new("xcrun").arg("metal").arg("-c").arg(&path).arg("-o").arg(&air).output();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&air);
    match out {
        Err(e) => Some(Err(format!("xcrun: {e}"))),
        Ok(o) if o.status.success() => Some(Ok(())),
        Ok(o) => Some(Err(String::from_utf8_lossy(&o.stderr).into_owned())),
    }
}
