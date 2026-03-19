#![allow(unstable_name_collisions)]

use {
    makepad_micro_proc_macro::{TokenBuilder, TokenParser},
    proc_macro::{Delimiter, Span, TokenStream},
    std::{
        fmt::Write,
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    },
};

const SCRIPT_MOD_SECTION_NAME: &str = "makepad:script_mod";
const SCRIPT_MOD_RECORDS_DIR_ENV: &str = "MAKEPAD_EXTERNALIZED_SCRIPT_MOD_DIR";
static NEXT_SCRIPT_MOD_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Default)]
struct ScriptImplOptions {
    externalize_mod_source: bool,
}

fn script_mod_externalization_enabled() -> bool {
    std::env::var_os("MAKEPAD_EXTERNALIZE_SCRIPT_MODS").is_some()
}

fn normalize_record_relative_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn externalized_script_mod_file_key(span: Span) -> String {
    let package_name = std::env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "unknown".to_string());
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from);

    let relative_path = if let (Some(local_file), Some(manifest_dir)) = (span.local_file(), manifest_dir)
    {
        local_file
            .strip_prefix(&manifest_dir)
            .map(PathBuf::from)
            .unwrap_or(local_file)
    } else {
        PathBuf::from(span.file())
    };

    format!(
        "{package_name}/{}",
        normalize_record_relative_path(&relative_path)
            .trim_start_matches("./")
            .trim_start_matches('/')
    )
}

fn stable_u64_hash(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn next_script_mod_source_id(file: &str, line: usize, column: usize, code: &str) -> u64 {
    let nonce = NEXT_SCRIPT_MOD_NONCE.fetch_add(1, Ordering::Relaxed);
    let mut payload = Vec::new();
    payload.extend_from_slice(file.as_bytes());
    payload.extend_from_slice(&(line as u64).to_le_bytes());
    payload.extend_from_slice(&(column as u64).to_le_bytes());
    payload.extend_from_slice(&nonce.to_le_bytes());
    payload.extend_from_slice(code.as_bytes());
    stable_u64_hash(&payload)
}

fn externalized_script_mod_record_path(source_id: u64) -> PathBuf {
    let output_dir = std::env::var_os(SCRIPT_MOD_RECORDS_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "{SCRIPT_MOD_RECORDS_DIR_ENV} must be set when MAKEPAD_EXTERNALIZE_SCRIPT_MODS is enabled"
            )
        });
    output_dir.join(format!("{source_id:016x}.txt"))
}

fn write_externalized_script_mod_record(source_id: u64, code: &str) {
    let record_path = externalized_script_mod_record_path(source_id);
    if let Some(parent) = record_path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!(
                "Failed to create externalized script_mod directory {:?}: {error}",
                parent
            )
        });
    }
    fs::write(&record_path, code).unwrap_or_else(|error| {
        panic!(
            "Failed to write externalized script_mod record {:?}: {error}",
            record_path
        )
    });
}

pub fn script_mod_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let ts = script_impl_with_options(
        input,
        ScriptImplOptions {
            externalize_mod_source: script_mod_externalization_enabled(),
        },
    );
    tb.add("pub fn script_mod(vm:&mut ScriptVm)->ScriptValue{");
    tb.add("    let sb=").stream(Some(ts)).add(";");
    tb.add("    vm.eval(sb)");
    tb.add("}");
    tb.end()
}

pub fn script_apply_eval_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();

    // Parse: cx, target, { script code }
    // First get the cx expression
    let cx_expr = parser.eat_any_ident().expect("Expected cx identifier");
    parser.eat_punct_alone(',');

    // Get the target expression (could be self.draw_bg or similar)
    let mut target_tb = TokenBuilder::new();
    while !parser.eat_punct_alone(',') {
        if let Some(tt) = parser.current.clone() {
            target_tb.extend(tt);
            parser.advance();
        } else {
            break;
        }
    }
    let target_stream = target_tb.end();

    // The rest is the script code (already includes braces from eat_level)
    // Prepend __script_source__ to make it __script_source__{...}
    let script_code: TokenStream = {
        let mut tb = TokenBuilder::new();
        tb.add("__script_source__");
        tb.stream(Some(parser.eat_level()));
        tb.end()
    };

    // Generate the script_impl output (the ScriptMod struct)
    // Use script_impl_expr to NOT add semicolon - we want to return the expression value
    let script_mod = script_impl_with_options(script_code, ScriptImplOptions::default());

    // Build: cx.with_vm(|vm| { let script = ScriptMod{...}; target.script_apply_eval(vm, script) })
    tb.ident(&cx_expr).add(".with_vm(|vm|{");
    tb.add("let script =").stream(Some(script_mod)).add(";");
    tb.stream(Some(target_stream));
    tb.add(".script_apply_eval(vm, script)");
    tb.add("})");

    tb.end()
}

pub fn script_impl(input: TokenStream) -> TokenStream {
    script_impl_with_options(input, ScriptImplOptions::default())
}

fn script_impl_with_options(input: TokenStream, options: ScriptImplOptions) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();

    if let Some(span) = parser.span() {
        let (s, values) = token_parser_to_whitespace_matching_string(&mut parser, span);
        let start_line = span.start().line();
        let start_column = span.start().column();
        let externalized_file_key = externalized_script_mod_file_key(span);
        let source_id =
            next_script_mod_source_id(&externalized_file_key, start_line, start_column, &s);
        let externalized_file_key = options
            .externalize_mod_source
            .then_some(externalized_file_key.clone());
        let file_expr = if let Some(file_key) = externalized_file_key.as_ref() {
            format!("{file_key:?}.to_string()")
        } else {
            "file!().to_string().replace(\"\\\\\", \"/\")"
                .to_string()
        };
        let inline_code = if options.externalize_mod_source {
            ""
        } else {
            &s
        };

        if options.externalize_mod_source {
            let file_key = externalized_file_key
                .as_ref()
                .expect("externalized script_mod file key");
            write_externalized_script_mod_record(source_id, &s);
            tb.add("{");
            tb.add("#[cfg(target_arch = \"wasm32\")]");
            tb.add("#[allow(long_running_const_eval)]");
            tb.add("const _ : () = {");
            tb.add("const FILE: &str = ")
                .string(file_key)
                .add(";");
            tb.add("const MODULE_PATH: &str = module_path!();");
            tb.add(&format!("const LINE: u32 = {start_line};"));
            tb.add(&format!("const COLUMN: u32 = {start_column};"));
            tb.add(&format!("const SOURCE_ID: u64 = {source_id};"));
            tb.add("const LEN: usize = 28 + FILE.len() + MODULE_PATH.len();");
            tb.add("const fn pack() -> [u8; LEN] {");
            tb.add("let file_bytes = FILE.as_bytes();");
            tb.add("let module_path_bytes = MODULE_PATH.as_bytes();");
            tb.add("let mut out = [0; LEN];");
            tb.add("let file_len = file_bytes.len() as u32;");
            tb.add("let module_path_len = module_path_bytes.len() as u32;");
            tb.add("let file_len_bytes = file_len.to_le_bytes();");
            tb.add("let line_bytes = LINE.to_le_bytes();");
            tb.add("let column_bytes = COLUMN.to_le_bytes();");
            tb.add("let module_path_len_bytes = module_path_len.to_le_bytes();");
            tb.add("let source_id_bytes = SOURCE_ID.to_le_bytes();");
            tb.add("out[0] = file_len_bytes[0];");
            tb.add("out[1] = file_len_bytes[1];");
            tb.add("out[2] = file_len_bytes[2];");
            tb.add("out[3] = file_len_bytes[3];");
            tb.add("out[4] = line_bytes[0];");
            tb.add("out[5] = line_bytes[1];");
            tb.add("out[6] = line_bytes[2];");
            tb.add("out[7] = line_bytes[3];");
            tb.add("out[8] = column_bytes[0];");
            tb.add("out[9] = column_bytes[1];");
            tb.add("out[10] = column_bytes[2];");
            tb.add("out[11] = column_bytes[3];");
            tb.add("out[12] = module_path_len_bytes[0];");
            tb.add("out[13] = module_path_len_bytes[1];");
            tb.add("out[14] = module_path_len_bytes[2];");
            tb.add("out[15] = module_path_len_bytes[3];");
            tb.add("out[16] = source_id_bytes[0];");
            tb.add("out[17] = source_id_bytes[1];");
            tb.add("out[18] = source_id_bytes[2];");
            tb.add("out[19] = source_id_bytes[3];");
            tb.add("out[20] = source_id_bytes[4];");
            tb.add("out[21] = source_id_bytes[5];");
            tb.add("out[22] = source_id_bytes[6];");
            tb.add("out[23] = source_id_bytes[7];");
            tb.add("out[24] = 0;");
            tb.add("out[25] = 0;");
            tb.add("out[26] = 0;");
            tb.add("out[27] = 0;");
            tb.add("let mut i = 0;");
            tb.add("while i < file_bytes.len() {");
            tb.add("let byte = file_bytes[i];");
            tb.add("out[28 + i] = if byte == 92 { 47 } else { byte };");
            tb.add("i += 1;");
            tb.add("}");
            tb.add("let mut j = 0;");
            tb.add("while j < module_path_bytes.len() {");
            tb.add("out[28 + file_bytes.len() + j] = module_path_bytes[j];");
            tb.add("j += 1;");
            tb.add("}");
            tb.add("out");
            tb.add("}");
            tb.add("#[used]");
            tb.add("#[unsafe(link_section = ")
                .string(SCRIPT_MOD_SECTION_NAME)
                .add(")]");
            tb.add("static SCRIPT_MOD_SECTION: [u8; LEN] = pack();");
            tb.add("};");
        }

        tb.add("ScriptMod {");
        tb.add("    cargo_manifest_path: env!(")
            .string("CARGO_MANIFEST_DIR")
            .add(").trim_start_matches(")
            .string("\\\\?\\")
            .add(").to_string(),");
        tb.add("    module_path :")
            .ident_with_span("module_path", span)
            .add("!().to_string(),");
        tb.add(&format!("    source_id:{source_id},"));
        tb.add("    file:").add(&file_expr).add(",");
        tb.add(&format!("    line:{start_line},"));
        tb.add(&format!("    column:{start_column},"));

        tb.add("    code:").string(inline_code).add(".to_string(),");
        tb.add("    values:{");
        tb.add("        let mut v = Vec::new();");
        for value in &values {
            tb.add("v.push( {")
                .stream(Some(value.clone()))
                .add("}.script_to_value(vm) );");
        }
        tb.add("    v}");
        tb.add("}");
        if options.externalize_mod_source {
            tb.add("}");
        }
    } else {
        tb.add("ScriptMod::default()");
    }
    tb.end()
}

// sself function parses tokens into a source-equal whitespaced output string
fn token_parser_to_whitespace_matching_string(
    parser: &mut TokenParser,
    span: Span,
) -> (String, Vec<TokenStream>) {
    let mut s = String::new();
    let mut values = Vec::new();

    tp_to_str(parser, span, &mut s, &mut values, &mut None);
    s.push(';');
    return (s, values);

    #[derive(Clone, Copy)]
    struct Lc {
        line: usize,
        column: usize,
    }

    impl Lc {
        fn _next_char(self) -> Self {
            Self {
                line: self.line,
                column: self.column + 1,
            }
        }
    }

    fn delim_to_pair(delim: Delimiter) -> (char, char) {
        match delim {
            Delimiter::Brace => ('{', '}'),
            Delimiter::Parenthesis => ('(', ')'),
            Delimiter::Bracket => ('[', ']'),
            Delimiter::None => (' ', ' '),
        }
    }

    fn tp_to_str(
        parser: &mut TokenParser,
        span: Span,
        out: &mut String,
        values: &mut Vec<TokenStream>,
        last_end: &mut Option<Lc>,
    ) {
        fn lc_from_start(span: Span) -> Lc {
            Lc {
                line: span.start().line(),
                column: span.start().column(),
            }
        }

        fn lc_from_end(span: Span) -> Lc {
            Lc {
                line: span.end().line(),
                column: span.end().column(),
            }
        }

        fn delta_whitespace(now: Lc, needed: Lc, out: &mut String) {
            if now.line == needed.line {
                for _ in now.column..needed.column {
                    out.push(' ');
                }
            } else {
                for _ in now.line..needed.line {
                    out.push('\n');
                }
                for _ in 1..needed.column {
                    out.push(' ');
                }
            }
        }

        if last_end.is_none() {
            *last_end = Some(lc_from_start(span));
        }

        let mut last_tt = None;

        while !parser.eat_eot() {
            let span = parser.span().unwrap();
            if let Some(delim) = parser.open_group() {
                if let Some(TokenTree::Punct(last_punct)) = &last_tt {
                    if last_punct.as_char() == '#' {
                        last_tt = None;
                        out.pop();
                        let index = values.len();
                        write!(out, "#({index})").unwrap();
                        values.push(parser.eat_level());
                        continue;
                    }
                }

                let (gs, ge) = delim_to_pair(delim);
                let start = lc_from_start(span);
                let end = lc_from_end(span);
                delta_whitespace(last_end.unwrap(), start, out);
                out.push(gs);
                *last_end = Some(start._next_char());
                tp_to_str(parser, span, out, values, last_end);
                delta_whitespace(
                    last_end.unwrap(),
                    Lc {
                        line: end.line,
                        column: end.column - 1,
                    },
                    out,
                );
                *last_end = Some(end);
                out.push(ge);
            } else {
                if let Some(tt) = &parser.current {
                    {
                        last_tt = Some(tt.clone());
                        let start = lc_from_start(span);
                        delta_whitespace(last_end.unwrap(), start, out);
                    }

                    out.push_str(&tt.to_string());

                    *last_end = Some(lc_from_end(span));
                }
                parser.advance();
            }
        }
    }
}

use proc_macro::TokenTree;
