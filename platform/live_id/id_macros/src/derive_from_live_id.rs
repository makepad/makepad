use proc_macro::{TokenStream};

use makepad_macro_lib::{
    TokenBuilder,
    TokenParser,
};

pub fn derive_from_live_id_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            tb.add("impl");
            tb.add("From<LiveId> for").ident(&struct_name).add("{");
            tb.add("    fn from(live_id:LiveId)->").ident(&struct_name).add("{").ident(&struct_name).add("(live_id)}");
            tb.add("}");
            return tb.end();
        }
    }
    return parser.unexpected()
}

