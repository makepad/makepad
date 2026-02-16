use {
    crate::makepad_script::{
        shader::ShaderOutput,
        shader_wgsl::compile_draw_shader_wgsl_source,
        value::ScriptObject,
        vm::ScriptVm,
    },
    std::fmt::Write,
};

#[derive(Clone)]
pub struct CxVulkanShaderBinary {
    pub vertex_spirv: Option<Vec<u32>>,
    pub fragment_spirv: Option<Vec<u32>>,
    pub dyn_uniform_binding: u32,
    pub texture_binding_base: u32,
    pub sampler_binding_base: u32,
    pub geometry_slots: usize,
    pub instance_slots: usize,
}

fn compile_wgsl_to_spirv(wgsl: &str) -> Result<(Option<Vec<u32>>, Option<Vec<u32>>), String> {
    use naga::{back::spv, valid};

    fn extract_error_line(details: &str) -> Option<usize> {
        let marker = "wgsl:";
        let start = details.find(marker)? + marker.len();
        let rest = &details[start..];
        let end = rest.find(':')?;
        rest[..end].trim().parse::<usize>().ok()
    }

    fn wgsl_context(wgsl: &str, line: usize, radius: usize) -> String {
        let start = line.saturating_sub(radius).max(1);
        let end = line.saturating_add(radius);
        let mut out = String::new();
        for (i, src_line) in wgsl.lines().enumerate() {
            let ln = i + 1;
            if ln >= start && ln <= end {
                let _ = writeln!(out, "{ln:4} | {src_line}");
            }
        }
        out
    }

    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| {
        let details = e.emit_to_string(wgsl);
        let context = extract_error_line(&details)
            .map(|line| {
                format!(
                    "\nWGSL context around line {line}:\n{}",
                    wgsl_context(wgsl, line, 4)
                )
            })
            .unwrap_or_default();
        format!("WGSL parse error: {e}\n{details}{context}")
    })?;

    let mut validator =
        valid::Validator::new(valid::ValidationFlags::all(), valid::Capabilities::empty());
    let module_info = validator
        .validate(&module)
        .map_err(|e| format!("WGSL validation error: {e}"))?;

    let options = spv::Options {
        lang_version: (1, 3),
        flags: spv::WriterFlags::empty(),
        fake_missing_bindings: true,
        binding_map: spv::BindingMap::default(),
        capabilities: None,
        bounds_check_policies: naga::proc::BoundsCheckPolicies::default(),
        zero_initialize_workgroup_memory: spv::ZeroInitializeWorkgroupMemoryMode::None,
        force_loop_bounding: false,
        use_storage_input_output_16: false,
        debug_info: None,
    };

    let has_vertex = module
        .entry_points
        .iter()
        .any(|ep| ep.stage == naga::ShaderStage::Vertex && ep.name == "vertex_main");
    let has_fragment = module
        .entry_points
        .iter()
        .any(|ep| ep.stage == naga::ShaderStage::Fragment && ep.name == "fragment_main");

    if !has_vertex && !has_fragment {
        return Err("WGSL module has no entry points".to_string());
    }

    let vertex_spirv = if has_vertex {
        let pipeline = spv::PipelineOptions {
            shader_stage: naga::ShaderStage::Vertex,
            entry_point: "vertex_main".to_string(),
        };
        Some(
            spv::write_vec(&module, &module_info, &options, Some(&pipeline))
                .map_err(|e| format!("SPIR-V write failed for vertex_main: {e}"))?,
        )
    } else {
        None
    };

    let fragment_spirv = if has_fragment {
        let pipeline = spv::PipelineOptions {
            shader_stage: naga::ShaderStage::Fragment,
            entry_point: "fragment_main".to_string(),
        };
        Some(
            spv::write_vec(&module, &module_info, &options, Some(&pipeline))
                .map_err(|e| format!("SPIR-V write failed for fragment_main: {e}"))?,
        )
    } else {
        None
    };

    Ok((vertex_spirv, fragment_spirv))
}

pub(crate) fn compile_draw_shader_wgsl_to_spirv(
    vm: &mut ScriptVm,
    io_self: ScriptObject,
    layout_source: &ShaderOutput,
) -> Result<CxVulkanShaderBinary, String> {
    let wgsl_source = compile_draw_shader_wgsl_source(vm, io_self, layout_source)?;

    if std::env::var_os("MAKEPAD_DUMP_VULKAN_WGSL").is_some() {
        crate::log!("---- Vulkan WGSL ----\n{}", wgsl_source.wgsl);
    }

    let (vertex_spirv, fragment_spirv) = compile_wgsl_to_spirv(&wgsl_source.wgsl)
        .map_err(|err| format!("{err}\nSet MAKEPAD_DUMP_VULKAN_WGSL=1 to dump generated WGSL."))?;

    Ok(CxVulkanShaderBinary {
        vertex_spirv,
        fragment_spirv,
        dyn_uniform_binding: wgsl_source.dyn_uniform_binding,
        texture_binding_base: wgsl_source.texture_binding_base,
        sampler_binding_base: wgsl_source.sampler_binding_base,
        geometry_slots: wgsl_source.geometry_slots,
        instance_slots: wgsl_source.instance_slots,
    })
}
