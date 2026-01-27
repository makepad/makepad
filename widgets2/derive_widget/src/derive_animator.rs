use proc_macro::TokenStream;

use makepad_micro_proc_macro::{TokenBuilder, TokenParser, error};

pub fn derive_animator_impl(input: TokenStream) ->  TokenStream {
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
            return error("Unexpected type form")
        }
        else if let Some(fields) = parser.eat_all_struct_fields() {
            fields
        }
        else {
            return error("Unexpected field form")
        };
                
        // alright now. we have a field
        let animator_field = fields.iter().find( | field | field.attrs.iter().any( | a | a.name == "animator"));
        
        if let Some(animator_field) = animator_field {
                        
            tb.add("impl").stream(generic.clone());
            tb.add("AnimatorImpl for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
                        
            tb.add("    fn animator_play_scoped(&mut self, cx: &mut Cx, state: &[LiveId;2], scope:&mut Scope) {");
            tb.add("        if let Some(value) = self.").ident(&animator_field.name).add(".play(cx, state){");
            tb.add("            cx.with_vm(|vm| self.script_apply(vm, &Apply::Animate, scope, value));");
            tb.add("        }");
            tb.add("    }");
            
            tb.add("    fn animator_in_state(&self, cx: &Cx, check_state_pair: &[LiveId; 2]) -> bool{");
            tb.add("         self.").ident(&animator_field.name).add(".in_state(cx, check_state_pair)");
            tb.add("    }");
            
            tb.add("    fn animator_cut_scoped(&mut self, cx: &mut Cx, state: &[LiveId;2], scope:&mut Scope) {");
            tb.add("         if let Some(value) = self.").ident(&animator_field.name).add(".cut(cx, state){");
            tb.add("             cx.with_vm(|vm| self.script_apply(vm, &Apply::Animate, scope, value));");
            tb.add("         }");
            tb.add("    }");
           
            tb.add("    fn animator_handle_event_scoped(&mut self, cx: &mut Cx, event: &Event, scope:&mut Scope)->AnimatorAction{");
            tb.add("         let mut act = AnimatorAction::None;");
            tb.add("         if let Some(value) = self.").ident(&animator_field.name).add(".handle_event(cx, event, &mut act){");
            tb.add("             cx.with_vm(|vm| self.script_apply(vm, &Apply::Animate, scope, value));");
            tb.add("         }");
            tb.add("         act");
            tb.add("    }");
                                    
            tb.add("}");
        }
        return tb.end()
    }
    parser.unexpected() 
}
