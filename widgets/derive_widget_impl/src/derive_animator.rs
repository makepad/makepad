use makepad_micro_proc_macro::proc_macro::TokenStream;

use makepad_micro_proc_macro::{error, TokenBuilder, TokenParser};

pub fn derive_animator_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();

    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        let struct_name = parser.expect_any_ident().unwrap();
        let generic = parser.eat_generic();
        let types = parser.eat_all_types();
        let where_clause = parser.eat_where_clause(None); //Some("LiveUpdateHooks"));

        let fields = if let Some(_types) = types {
            return error("Unexpected type form");
        } else if let Some(fields) = parser.eat_all_struct_fields() {
            fields
        } else {
            return error("Unexpected field form");
        };

        // alright now. we have a field
        let animator_field = fields.iter().find(|field| field.name == "animator");

        if let Some(animator_field) = animator_field {
            // Check that we have a #[source] attribute to hold the reference to the object
            let has_source = fields
                .iter()
                .any(|field| field.attrs.iter().any(|attr| attr.name == "source"));

            if !has_source {
                tb.add("compile_error!(\"Animator derive requires a field with #[source] attribute to hold the ScriptObjectRef\");");
            }

            tb.add("impl").stream(generic.clone());
            tb.add("AnimatorImpl for")
                .ident(&struct_name)
                .stream(generic.clone())
                .stream(where_clause.clone())
                .add("{");

            tb.add("    fn animator_field(&self) -> &Animator { &self.")
                .ident(&animator_field.name)
                .add("}");
            tb.add("    fn animator_field_mut(&mut self) -> &mut Animator { &mut self.")
                .ident(&animator_field.name)
                .add("}");
            tb.add("}");
        }
        return tb.end();
    }
    parser.unexpected()
}
