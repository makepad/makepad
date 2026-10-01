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
pub mod data;
mod dce;
mod locals;
mod merge;
mod order;
mod panics;
mod peephole;
pub mod profile;
pub mod units;

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
    /// The module-wise strip (`units`): with a coverage run of this module,
    /// every unit the run never used is cut, before dead-code elimination.
    pub coverage: Option<units::Coverage>,
    /// The registration strip (`units::plan_modules`): with the script
    /// modules a collect run saw registered and used, the registration of
    /// every unused one is dropped, before dead-code elimination.
    pub modules: Option<units::ModuleUse>,
    /// Units kept whatever the coverage says (`crate::module`, `crate`, or
    /// a `prefix*`).
    pub keep_units: Vec<String>,
    /// Panics become traps (see `panics`): every call of a function that
    /// never returns becomes `unreachable`, and the panic machinery behind
    /// them goes. A panic then traps without its message; `symbols` keeps
    /// what each site said.
    pub panic_trap: bool,
    /// Build the symbol file (`OptimizeReport::symbols`): the output's
    /// function names and, with `panic_trap`, every trap site's code offset
    /// with its message and location, to read a trap's stack offline.
    pub symbols: bool,
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
            coverage: None,
            modules: None,
            keep_units: Vec::new(),
            panic_trap: false,
            symbols: false,
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
    /// The module-wise strip's units, when it ran.
    pub units: Option<String>,
    /// The symbol file, when `OptimizeOptions::symbols` asked for it.
    pub symbols: Option<String>,
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
        let after = shipped(module, opts);
        report.passes.push(OptimizePassReport {
            name,
            bytes_before: bytes.len(),
            bytes_after: after.len(),
            reverted,
        });
        *bytes = after;
    };

    // Before the names go: the units are named by them.
    let plan = match (&opts.coverage, &opts.modules) {
        (Some(coverage), _) => Some(units::plan(&module, coverage, &opts.keep_units)),
        (None, Some(modules)) => Some(units::plan_modules(&module, modules, &opts.keep_units)),
        (None, None) => None,
    };
    if let Some(plan) = plan {
        let cleared = std::cell::Cell::new(0);
        run("units", &mut module, &mut bytes, &|module| cleared.set(units::strip(module, &plan)));
        report.units = Some(format!("{}data cleared: {} bytes\n", plan.to_text(), cleared.get()));
    }
    if opts.strip {
        run("strip", &mut module, &mut bytes, &|module| {
            strip(module, opts)
        });
    }
    let sites = std::cell::RefCell::new(Vec::new());
    // What the panic sites point at in the data, cleared once they are gone.
    let panic_data = if opts.panic_trap { data::Objects::find(&module) } else { data::Objects::default() };
    if opts.panic_trap {
        run("panics", &mut module, &mut bytes, &|module| {
            *sites.borrow_mut() = panics::run(module, opts.symbols)
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
    if panic_data.len() > 0 {
        run("data", &mut module, &mut bytes, &|module| {
            data::clear_dead_objects(module, &panic_data);
        });
    }
    if opts.compact {
        run("compact", &mut module, &mut bytes, &compact::run);
    }
    if opts.order {
        run("order", &mut module, &mut bytes, &order::run);
    }

    let sites = sites.into_inner();
    let mut traps = if sites.is_empty() { Vec::new() } else { panics::unmark(&mut module, sites.len()) };
    if !traps.is_empty() {
        // Functions that differed only in their site markers are equal now;
        // a merged function's traps sit where its copy's do.
        if opts.merge {
            let moved = merge::run_mapped(&mut module);
            for (func, _, _) in &mut traps {
                *func = moved[*func as usize];
            }
        }
        bytes = shipped(&mut module, opts);
    }
    if opts.symbols {
        report.symbols = Some(symbols(&module, &bytes, &sites, &traps)?);
    }
    if !opts.keep_names {
        module.names = None;
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

/// The module with a coverage probe at every function entry (see
/// `units`): an exported memory, `units::COVERAGE_EXPORT`, holding a phase
/// byte per function index. Names are kept.
pub fn wasm_instrument_coverage(buf: &[u8], phase_marks: &[String]) -> Result<Vec<u8>, String> {
    let mut module = ir::decode(buf)?;
    units::instrument(&mut module, phase_marks)?;
    ir::validate_module(&module).map_err(|msg| format!("instrumented module does not validate: {msg}"))?;
    Ok(encode::encode(&module))
}

/// The functions an instrumented module's run entered, from the bytes of
/// its coverage memory.
pub fn wasm_coverage(instrumented: &[u8], memory: &[u8]) -> Result<units::Coverage, String> {
    Ok(units::Coverage::from_memory(&ir::decode(instrumented)?, memory))
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
    // The symbol file is built from the names at the end; they go then.
    if !opts.keep_names && !opts.symbols {
        module.names = None;
    }
}

/// The bytes that ship: without the names a symbol file alone kept.
fn shipped(module: &mut ir::Module, opts: &OptimizeOptions) -> Vec<u8> {
    if opts.keep_names {
        return encode::encode(module);
    }
    let names = module.names.take();
    let bytes = encode::encode(module);
    module.names = names;
    bytes
}

/// The symbol file: every function of `bytes` by index with its demangled
/// name, then every panic site's trap by its code offset (the offset a
/// browser's wasm stack frame shows) with what the panic said.
fn symbols(
    module: &ir::Module,
    bytes: &[u8],
    sites: &[panics::Site],
    traps: &[(u32, usize, usize)],
) -> Res<String> {
    use std::fmt::Write;
    let mut out = String::from(
        "# wasm symbols\n# F <function index> <name>\n# P <code offset> <function index> <callee> | <message> | <location>\n",
    );
    let names: std::collections::HashMap<u32, &str> = module
        .names
        .iter()
        .flat_map(|names| names.funcs.iter().map(|(i, s)| (*i, s.as_str())))
        .collect();
    let total = module.num_imported_funcs() as usize + module.funcs.len();
    for index in 0..total as u32 {
        if let Some(symbol) = names.get(&index) {
            let name = demangle::demangle(symbol).map(|d| d.name).unwrap_or_else(|| symbol.to_string());
            let _ = writeln!(out, "F {index} {name}");
        }
    }
    if traps.is_empty() {
        return Ok(out);
    }
    let (_, layout) = ir::Module::decode_with_layout(bytes, ir::Features::default()).map_err(|e| e.to_string())?;
    let imported = module.num_imported_funcs();
    let mut offsets: std::collections::HashMap<u32, Vec<usize>> = std::collections::HashMap::new();
    let one_line = |text: &str| text.replace('\n', "\\n").replace('|', "/");
    for &(func, pos, site) in traps {
        let entry = layout.funcs[(func - imported) as usize].clone();
        if !offsets.contains_key(&func) {
            let within = ir::body_offsets(&bytes[entry.clone()], ir::Features::default()).map_err(|e| e.to_string())?;
            offsets.insert(func, within);
        }
        let at = entry.start + offsets[&func][pos];
        let site = &sites[site];
        let _ = writeln!(
            out,
            "P 0x{at:x} {func} {} | {} | {}",
            one_line(&site.callee),
            site.message.as_deref().map(one_line).unwrap_or_default(),
            site.location.as_deref().unwrap_or("")
        );
    }
    Ok(out)
}
