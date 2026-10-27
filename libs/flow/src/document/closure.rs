//! A definition's module closure: the pinned component modules a root flow
//! evaluates with, in dependency order, immutable once built.
//!
//! The closure travels with the definition through every path that
//! evaluates it (load, graph write, revert, run), so a revision is always
//! the same root and the same modules. Order is resolved from each module's
//! own `use mod.components.<Alias>` statements, never from the manifest's
//! array order; unknown aliases and cycles are refused where they are
//! written. Every budget here is checked before anything grows past it.

use crate::graph::MAX_SOURCE_BYTES;
pub use crate::wire::ModuleSource;
use crate::EvalError;
use makepad_script::tokenizer::{ScriptToken, ScriptTokenPos, ScriptTokenizer};
use makepad_script::ScriptVmBase;
use std::collections::{HashMap, HashSet};

/// Most bytes every module of one closure may hold together.
pub const MAX_CLOSURE_BYTES: usize = 16 * MAX_SOURCE_BYTES;
/// Most modules one closure may pin.
pub const MAX_MODULES: usize = 64;
/// Longest module path or alias.
pub const MAX_MODULE_NAME: usize = 256;
/// Deepest container nesting a lowering follows.
pub const MAX_COMPONENT_DEPTH: usize = 8;
/// Most component instances (module evaluations) one root may expand to.
pub const MAX_INSTANCES: usize = 256;
/// Most nodes and edges a lowered graph may hold.
pub const MAX_LOWERED_NODES: usize = 4_096;
pub const MAX_LOWERED_EDGES: usize = 16_384;

/// The Splash module every pinned component is installed under.
pub const COMPONENTS_MODULE: &str = "components";

/// Pinned modules in dependency order: a module comes after every module
/// it names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Closure {
    pub modules: Vec<ModuleSource>,
}

/// One `use mod.components.…` a module wrote: a named alias, or a glob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UseRef {
    /// `None` for `use mod.components.*`.
    pub alias: Option<String>,
    pub line: u32,
    pub col: u32,
}

impl Closure {
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.modules.iter().map(|module| module.source.len()).sum()
    }

    pub fn module(&self, alias: &str) -> Option<&ModuleSource> {
        self.modules.iter().find(|module| module.alias == alias)
    }

    /// Order `modules` by their declared dependencies and check the byte
    /// budget. Aliases must be unique; every named alias must be pinned; a
    /// self or mutual cycle is refused at the `use` that closes it; a glob
    /// import is allowed only in a module no other module depends on (it
    /// would otherwise depend on everything, itself included).
    pub fn ordered(modules: Vec<ModuleSource>) -> Result<Self, EvalError> {
        // Every wire and package caller passes here: the count, names,
        // sizes, hashes and identities of the modules are checked before
        // anything is resolved or evaluated, whoever supplied them.
        if modules.len() > MAX_MODULES {
            return Err(EvalError {
                file: modules.first().map(|module| module.path.clone()).unwrap_or_default(),
                line: 1,
                col: 1,
                message: format!("{} modules are pinned, over the {MAX_MODULES} limit", modules.len()),
            });
        }
        let mut total = 0usize;
        let mut seen = HashSet::new();
        let mut paths = HashSet::new();
        for module in &modules {
            if module.path.is_empty() || module.path.len() > MAX_MODULE_NAME || module.alias.len() > MAX_MODULE_NAME {
                return Err(EvalError {
                    file: module.path.clone(),
                    line: 1,
                    col: 1,
                    message: "module path or alias is empty or longer than the limit".to_string(),
                });
            }
            if module.alias.is_empty() || !is_alias(&module.alias) {
                return Err(EvalError {
                    file: module.path.clone(),
                    line: 1,
                    col: 1,
                    message: format!("module alias `{}` is not an identifier", module.alias),
                });
            }
            if !seen.insert(module.alias.as_str()) {
                return Err(EvalError {
                    file: module.path.clone(),
                    line: 1,
                    col: 1,
                    message: format!("module alias `{}` is pinned twice", module.alias),
                });
            }
            if !paths.insert(module.path.as_str()) {
                // Two sources under one file identity would share the VM's
                // body cache; refused rather than let one replace the other.
                return Err(EvalError {
                    file: module.path.clone(),
                    line: 1,
                    col: 1,
                    message: format!("module path `{}` is pinned twice", module.path),
                });
            }
            if module.source.len() > MAX_SOURCE_BYTES {
                return Err(EvalError {
                    file: module.path.clone(),
                    line: 1,
                    col: 1,
                    message: format!("module is {} bytes, over the {MAX_SOURCE_BYTES} byte source limit", module.source.len()),
                });
            }
            total += module.source.len();
            if total > MAX_CLOSURE_BYTES {
                return Err(EvalError {
                    file: module.path.clone(),
                    line: 1,
                    col: 1,
                    message: format!("the modules hold more than {MAX_CLOSURE_BYTES} bytes together"),
                });
            }
            let found = source_hash(&module.source);
            if found != module.hash {
                return Err(EvalError {
                    file: module.path.clone(),
                    line: 1,
                    col: 1,
                    message: format!("module does not match its pinned hash (expected {}, found {found})", module.hash),
                });
            }
        }
        let index: HashMap<&str, usize> =
            modules.iter().enumerate().map(|(i, module)| (module.alias.as_str(), i)).collect();
        // Dependencies per module, with the `use` that declared each.
        let mut deps: Vec<Vec<(usize, UseRef)>> = Vec::with_capacity(modules.len());
        for module in &modules {
            let mut own = Vec::new();
            for used in scan_uses(&module.source) {
                match &used.alias {
                    // A reusable module names what it depends on; a glob
                    // would make every alias a dependency, used or not.
                    None => {
                        return Err(EvalError {
                            file: module.path.clone(),
                            line: used.line,
                            col: used.col,
                            message: "a component imports other components by name (`use mod.components.<Alias>`), not with `*`"
                                .to_string(),
                        })
                    }
                    Some(alias) => match index.get(alias.as_str()) {
                        Some(&target) => own.push((target, used.clone())),
                        None => {
                            return Err(EvalError {
                                file: module.path.clone(),
                                line: used.line,
                                col: used.col,
                                message: format!(
                                    "unknown component `{alias}`; pin it in app.splash under `modules`"
                                ),
                            })
                        }
                    },
                }
            }
            deps.push(own);
        }
        // Cycles: a depth-first walk that names the chain at the `use`
        // closing it.
        let mut state = vec![0u8; modules.len()]; // 0 unvisited, 1 on stack, 2 done
        let mut stack: Vec<usize> = Vec::new();
        fn visit(
            node: usize,
            deps: &[Vec<(usize, UseRef)>],
            modules: &[ModuleSource],
            state: &mut [u8],
            stack: &mut Vec<usize>,
        ) -> Result<(), EvalError> {
            state[node] = 1;
            stack.push(node);
            for (target, used) in &deps[node] {
                match state[*target] {
                    1 => {
                        let start = stack.iter().position(|entry| entry == target).unwrap_or(0);
                        let chain: Vec<&str> = stack[start..]
                            .iter()
                            .map(|entry| modules[*entry].alias.as_str())
                            .chain(std::iter::once(modules[*target].alias.as_str()))
                            .collect();
                        return Err(EvalError {
                            file: modules[node].path.clone(),
                            line: used.line,
                            col: used.col,
                            message: format!("component imports form a cycle: {}", chain.join(" → ")),
                        });
                    }
                    0 => visit(*target, deps, modules, state, stack)?,
                    _ => {}
                }
            }
            stack.pop();
            state[node] = 2;
            Ok(())
        }
        for i in 0..modules.len() {
            if state[i] == 0 {
                visit(i, &deps, &modules, &mut state, &mut stack)?;
            }
        }
        // Topological order: repeatedly take a module whose dependencies
        // are all placed; the cycle check above guarantees progress.
        let mut placed = vec![false; modules.len()];
        let mut order = Vec::with_capacity(modules.len());
        while order.len() < modules.len() {
            let next = (0..modules.len())
                .find(|&i| !placed[i] && deps[i].iter().all(|(target, _)| placed[*target]))
                .expect("acyclic dependency graph has a placeable module");
            placed[next] = true;
            order.push(next);
        }
        Ok(Self { modules: order.into_iter().map(|i| modules[i].clone()).collect() })
    }
}

/// The pin of a module: sha256 of its bytes, lower-case hex.
pub fn source_hash(source: &str) -> String {
    let mut sha = crate::digest::Sha256::new();
    sha.update(source.as_bytes());
    crate::digest::to_hex(&sha.finish())
}

fn is_alias(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Every `use mod.components.<Alias>` / `use mod.components.*` statement in
/// `source`, read from Splash's own tokens: comments (nested block comments
/// included) and strings are what the tokenizer says they are, so nothing
/// inside them can name an import. A `use` is taken wherever the parser
/// would take one (the path may continue on following lines, as the parser
/// allows, and a `use` inside a block counts too), so no dependency is
/// missed and left to manifest order.
pub fn scan_uses(source: &str) -> Vec<UseRef> {
    let mut base = ScriptVmBase::new();
    let mut tokenizer = ScriptTokenizer::default();
    let tokens: Vec<ScriptTokenPos> = tokenizer.tokenize(source, base.heap_mut()).to_vec();
    let name = |token: &ScriptToken| -> Option<String> {
        token.as_identifier().or_else(|| token.operator_or_separator())
            .and_then(|id| id.as_string(|text| text.map(str::to_string)))
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let token = &tokens[i];
        let after_path_segment = i > 0
            && tokens[i - 1].token().is_identifier()
            && !tokens[i - 1].preceded_by_newline()
            && i > 1
            && name(&tokens[i - 2].token()).as_deref() == Some(".");
        if !after_path_segment && token.token().is_identifier() && name(&token.token()).as_deref() == Some("use") {
            let (line, col) = tokenizer.token_index_to_row_col(i as u32).unwrap_or((0, 0));
            // The path: identifiers and `*` joined by `.`, until a token that
            // is neither. Line breaks inside the path are the parser's
            // continued expression and do not end it.
            let mut segments: Vec<String> = Vec::new();
            let mut j = i + 1;
            let mut expect_segment = true;
            while j < tokens.len() {
                let next = &tokens[j];
                let text = name(&next.token());
                match (expect_segment, text.as_deref()) {
                    (true, Some("*")) => {
                        segments.push("*".to_string());
                        expect_segment = false;
                    }
                    (true, Some(segment)) if next.token().is_identifier() => {
                        segments.push(segment.to_string());
                        expect_segment = false;
                    }
                    (false, Some(".")) => expect_segment = true,
                    _ => break,
                }
                j += 1;
            }
            if segments.len() == 3 && segments[0] == "mod" && segments[1] == COMPONENTS_MODULE {
                out.push(UseRef {
                    alias: (segments[2] != "*").then(|| segments[2].clone()),
                    line: line + 1,
                    col: col + 1,
                });
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
    out
}
