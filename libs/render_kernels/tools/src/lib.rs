//! The compute-kernel tools' text and shape: `kernel_guide` and
//! `kernel_check`, defined once and listed byte for byte the same by every
//! app's AI registry (Stage's studio API, Sandbox's game chat).
//! Dependency-free, so a light registry can list them; running them (and
//! the guide's text) is `makepad-render-kernels`' `tools` module.

/// One tool as every registry declares it. `input` and `output` are JSON
/// Schema (strict JSON); `args_doc` is a call sketch for registries that
/// show one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KernelToolDef {
    /// The dotted canonical name (`kernel.check`).
    pub name: &'static str,
    /// The underscore spelling (`kernel_check`): what the model sees.
    pub api_name: &'static str,
    pub description: &'static str,
    pub input: &'static str,
    pub output: &'static str,
    pub args_doc: &'static str,
}

pub const GUIDE: KernelToolDef = KernelToolDef {
    name: "kernel.guide",
    api_name: "kernel_guide",
    description: "How to write a compute kernel: fast Splash for per-vertex, per-instance, per-particle and per-glyph work (entries, buffers, layouts, emit, params, math modes, the stdlib and modules, host components, budgets and the traps). Read it once before writing a Kernel.",
    input: r#"{"type": "object", "properties": {}}"#,
    output: r#"{"type": "object", "additionalProperties": true, "properties": {"guide": {"type": "string", "description": "the kernel guide"}}, "required": ["guide"]}"#,
    args_doc: r#"{}"#,
};

pub const CHECK: KernelToolDef = KernelToolDef {
    name: "kernel.check",
    api_name: "kernel_check",
    description: "Check a compute kernel's Splash source without running it in the document: every compile problem with its line, column and the line's text; once it compiles, what it is (entry, buffers, params, math mode, worst-case ops per element, whether it runs in parallel and four-wide, estimated time for 1M elements) and a dry run over a few synthetic elements (inputs filled with a ramp, or with `inputs` you give) reporting NaN or infinite outputs, emit overflow, host-component errors and ns per element. `layouts` declares the host records the kernel writes (from the draw shader's reflection), `modules` the library modules it imports.",
    input: r#"{"type": "object", "properties": {"source": {"type": "string", "description": "the kernel's Splash source"}, "layouts": {"type": "array", "items": {"type": "object", "description": "{name, stride (words), fields: [{name, type: f32|i32|vec2|vec3|vec4|mat4, offset (words)}]}"}, "description": "record layouts the kernel names (output(Name), emit_buffer(Name, n))"}, "modules": {"type": "array", "items": {"type": "object", "description": "{path, source}"}, "description": "modules the kernel imports with `use`"}, "count": {"type": "integer", "minimum": 1, "maximum": 4096, "description": "elements in the dry run (default 64)"}, "params": {"type": "object", "additionalProperties": {"type": "number"}, "description": "params to set for the dry run"}, "inputs": {"type": "object", "additionalProperties": {"type": "array", "items": {"type": "number"}}, "description": "input buffers' words for the dry run (floats; default a ramp)"}}, "required": ["source"]}"#,
    output: r#"{"type": "object", "additionalProperties": true, "properties": {"ok": {"type": "boolean", "description": "it compiles and the dry run found nothing"}, "problems": {"type": "array", "items": {"type": "object", "description": "{line, col, message, text}"}, "description": "compile problems, or what the dry run found"}}, "required": ["ok", "problems"]}"#,
    args_doc: r#"{"source": "let pos = output(vec3)\nfn vertex(i) { pos[i] = vec3(float(i), 0.0, 0.0) }", "count": 64}"#,
};

/// Every kernel tool, in the order registries list them.
pub const TOOLS: [KernelToolDef; 2] = [GUIDE, CHECK];

/// The kernel tool named `name`, dotted or underscored.
pub fn find(name: &str) -> Option<&'static KernelToolDef> {
    TOOLS.iter().find(|tool| tool.name == name || tool.api_name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_legal_and_schemas_parse() {
        for tool in TOOLS {
            assert_eq!(tool.api_name, tool.name.replace('.', "_"));
            assert!(tool.api_name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{}", tool.api_name);
            for schema in [tool.input, tool.output, tool.args_doc] {
                makepad_strict_json::parse(schema.as_bytes()).unwrap_or_else(|e| panic!("{}: {e}: {schema}", tool.name));
            }
            assert_eq!(find(tool.name), find(tool.api_name));
        }
    }
}
