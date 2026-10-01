//! Makepad's own wasm optimiser: the last step of a web export, run on the
//! module rustc/LLD linked. It decodes the whole module, runs size passes on
//! it, and re-validates after every pass; a pass whose output does not
//! validate is undone and named in the report, so the result is always a
//! valid module.

pub mod demangle;
pub mod encode;
pub mod ir;
pub mod remap;

mod compact;
mod dce;
mod locals;
mod merge;
mod order;
mod peephole;
pub mod profile;

#[cfg(test)]
mod tests;

use crate::wasm_strip::WasmParseError;

pub(crate) type Res<T> = Result<T, String>;

#[derive(Clone, Debug)]
pub struct OptimizeOptions {
    /// Keep the function and global names of the `name` section (renumbered
    /// with the module) for debugging and profiling.
    pub keep_names: bool,
    /// Custom sections to keep, by exact name. Every other custom section is
    /// stripped.
    pub keep_custom_sections: Vec<String>,
    pub strip: bool,
    pub dce: bool,
    pub peephole: bool,
    pub locals: bool,
    pub merge: bool,
    pub compact: bool,
    pub order: bool,
    /// Validate after every pass and undo a pass whose result is invalid.
    /// The encoded result is always decoded and validated again; when it
    /// does not validate, optimising fails.
    pub validate_each_pass: bool,
}

impl Default for OptimizeOptions {
    fn default() -> OptimizeOptions {
        OptimizeOptions {
            keep_names: false,
            keep_custom_sections: Vec::new(),
            strip: true,
            dce: true,
            peephole: true,
            locals: true,
            merge: true,
            compact: true,
            order: true,
            validate_each_pass: true,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OptimizePassReport {
    pub name: &'static str,
    pub bytes_before: usize,
    pub bytes_after: usize,
    /// Set when the pass produced an invalid module and was undone.
    pub reverted: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OptimizeReport {
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub functions_before: usize,
    pub functions_after: usize,
    pub passes: Vec<OptimizePassReport>,
}

impl OptimizeReport {
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "wasm optimize: {} -> {} bytes ({:.1}%), functions {} -> {}\n",
            self.input_bytes,
            self.output_bytes,
            percent(self.input_bytes, self.output_bytes),
            self.functions_before,
            self.functions_after
        );
        for pass in &self.passes {
            out.push_str(&format!(
                "  {:<10} {:>10} -> {:>10}  {:>+9}",
                pass.name,
                pass.bytes_before,
                pass.bytes_after,
                pass.bytes_after as i64 - pass.bytes_before as i64
            ));
            if let Some(reason) = &pass.reverted {
                out.push_str(&format!("  REVERTED: {reason}"));
            }
            out.push('\n');
        }
        out
    }
}

fn percent(before: usize, after: usize) -> f64 {
    if before == 0 {
        return 0.0;
    }
    (after as f64 - before as f64) * 100.0 / before as f64
}

/// Checks that `buf` is a module the optimiser understands and that it is
/// valid, with the reason when it is not.
pub fn wasm_validate(buf: &[u8]) -> Result<(), String> {
    ir::validate_module(&ir::decode(buf)?)
}

/// The detailed form of `wasm_optimize`: the error says why the input was
/// refused.
pub fn wasm_optimize_checked(
    buf: &[u8],
    opts: &OptimizeOptions,
) -> Result<(Vec<u8>, OptimizeReport), String> {
    let mut module = ir::decode(buf)?;
    ir::validate_module(&module).map_err(|msg| format!("input does not validate: {msg}"))?;
    let mut report = OptimizeReport {
        input_bytes: buf.len(),
        functions_before: module.funcs.len() + module.num_imported_funcs() as usize,
        ..OptimizeReport::default()
    };

    let mut bytes = encode::encode(&module);
    report.passes.push(OptimizePassReport {
        name: "leb",
        bytes_before: buf.len(),
        bytes_after: bytes.len(),
        reverted: None,
    });

    let mut run = |name: &'static str,
                   module: &mut ir::Module,
                   bytes: &mut Vec<u8>,
                   pass: &dyn Fn(&mut ir::Module)| {
        let backup = opts.validate_each_pass.then(|| module.clone());
        pass(module);
        let mut reverted = None;
        if let Some(backup) = backup {
            if let Err(msg) = ir::validate_module(module) {
                *module = backup;
                reverted = Some(msg);
            }
        }
        let after = encode::encode(module);
        report.passes.push(OptimizePassReport {
            name,
            bytes_before: bytes.len(),
            bytes_after: after.len(),
            reverted,
        });
        *bytes = after;
    };

    if opts.strip {
        run("strip", &mut module, &mut bytes, &|module| {
            strip(module, opts)
        });
    }
    if opts.dce {
        run("dce", &mut module, &mut bytes, &dce::run);
    }
    if opts.peephole {
        run("peephole", &mut module, &mut bytes, &peephole::run);
    }
    if opts.locals {
        run("locals", &mut module, &mut bytes, &locals::run);
    }
    if opts.peephole && opts.locals {
        run("peephole", &mut module, &mut bytes, &peephole::run);
    }
    if opts.merge {
        run("merge", &mut module, &mut bytes, &merge::run);
    }
    if opts.dce && (opts.merge || opts.peephole) {
        run("dce", &mut module, &mut bytes, &dce::run);
    }
    if opts.compact {
        run("compact", &mut module, &mut bytes, &compact::run);
    }
    if opts.order {
        run("order", &mut module, &mut bytes, &order::run);
    }

    // The bytes that ship, decoded and validated again: every function's
    // operand stack and block types, and the encoder with them.
    if let Err(msg) = ir::decode(&bytes).and_then(|module| ir::validate_module(&module)) {
        return Err(format!("optimised module does not validate: {msg}"));
    }
    report.functions_after = module.funcs.len() + module.num_imported_funcs() as usize;
    report.output_bytes = bytes.len();
    Ok((bytes, report))
}

/// Optimises a linked wasm module for size. Fails only when the input cannot
/// be decoded or does not validate; the output always validates.
pub fn wasm_optimize(
    buf: &[u8],
    opts: &OptimizeOptions,
) -> Result<(Vec<u8>, OptimizeReport), WasmParseError> {
    wasm_optimize_checked(buf, opts).map_err(|_| WasmParseError)
}

fn strip(module: &mut ir::Module, opts: &OptimizeOptions) {
    module
        .customs
        .retain(|custom| opts.keep_custom_sections.iter().any(|keep| *keep == custom.name));
    if !opts.keep_names {
        module.names = None;
    }
}
