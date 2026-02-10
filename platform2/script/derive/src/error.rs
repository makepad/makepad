#![allow(unstable_name_collisions)]

use {
    makepad_micro_proc_macro::{TokenBuilder, TokenParser},
    proc_macro::TokenStream,
};

pub fn script_err_gen_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    // lets get the argument name
    let mut parser = TokenParser::new(input);
    fn parse(parser: &mut TokenParser, tb: &mut TokenBuilder) -> Result<(), TokenStream> {
        let ident = parser.expect_any_ident()?;
        tb.add("#[macro_export] macro_rules!")
            .ident(&ident)
            .add("{");
        tb.add("    ($trap:expr) => {");
        tb.add("        if let $crate::trap::ScriptTrap::Inner(trap) = $trap.pass(){");
        tb.add("            trap.push_err(ScriptValue::")
            .ident(&ident)
            .add("(trap.ip), stringify!($trap).into(), file!().into(), line!())");
        tb.add("        }else{");
        tb.add("            ScriptValue::")
            .ident(&ident)
            .add("(ScriptIp::default())");
        tb.add("        }");
        tb.add("    };");
        tb.add("    ($trap:expr, $($arg:tt)*) => {");
        tb.add("        if let $crate::trap::ScriptTrap::Inner(trap) = $trap.pass(){");
        tb.add("            trap.push_err(ScriptValue::")
            .ident(&ident)
            .add("(trap.ip), format!($($arg)*), file!().into(), line!())");
        tb.add("        }else{");
        tb.add("            ScriptValue::")
            .ident(&ident)
            .add("(ScriptIp::default())");
        tb.add("        }");
        tb.add("    }");
        tb.add("}");
        Ok(())
    }
    if let Err(e) = parse(&mut parser, &mut tb) {
        return e;
    };
    tb.end()
}
