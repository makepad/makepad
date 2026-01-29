

#![allow(unstable_name_collisions)]

use {
    proc_macro::{
        TokenStream,
    },
    makepad_micro_proc_macro::{TokenBuilder, TokenParser},
};

pub fn script_err_gen_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    // lets get the argument name
    let mut parser = TokenParser::new(input);
    fn parse(parser:&mut TokenParser, tb:&mut TokenBuilder)->Result<(),TokenStream>{
        let ident = parser.expect_any_ident()?;
        tb.add("#[macro_export] macro_rules!").ident(&ident).add("{");
        tb.add("    ($trap:expr) => {").ident(&ident).add("!($trap,").string(&ident).add(")};");
        tb.add("    ($trap:expr, $($arg:tt)*) => {");
        tb.add("        if let crate::trap::ScriptTrap::Inner(trap) = $trap.pass(){");
        tb.add("            let value = ScriptValue::").ident(&ident).add("(trap.ip);");
        tb.add("            trap.push_err(crate::trap::ScriptError{");
        tb.add("                in_rust: trap.in_rust,");
        tb.add("                value,");
        tb.add("                message: format!($($arg)*),");
        tb.add("                origin_file: file!().into(),");
        tb.add("                origin_line: line!(),");
        tb.add("            });");
        tb.add("            value");
        tb.add("        }else{");
        tb.add("            ScriptValue::").ident(&ident).add("(ScriptIp::default())");
        tb.add("        }");
        tb.add("    }");
        tb.add("}");
        Ok(())
    }
    if let Err(e) = parse(&mut parser, &mut tb){
        return e
    };
    tb.end()
}