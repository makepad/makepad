use proc_macro::{TokenStream};

use crate::macro_lib::*;
use crate::id::*;

pub fn derive_live_animate_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(None); //Some("LiveUpdateHooks"));
            
            let fields = if let Some(_types) = types {
                return parser.unexpected();
            }
            else if let Some(fields) = parser.eat_all_struct_fields() {
                fields
            }
            else {
                return parser.unexpected();
            };
            
            let animator = fields.iter().find( | field | field.name == "animator");
            
            if !animator.is_some() {
                // no can do
                return error("Cannot generate LiveAnimate without animator");
            }
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveAnimate for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn animate_to(&mut self, cx: &mut Cx, state_id: Id) {");
            tb.add("        if self.animator.state.is_none() {");
            tb.add("            self.animator.cut_to_live(cx, id!(state_default));");
            tb.add("         }");
            tb.add("        self.animator.animate_to_live(cx, state_id);");
            tb.add("    }");
            tb.add("    fn handle_animation(&mut self, cx: &mut Cx, event: &mut Event) {");
            tb.add("        if self.animator.do_animation(cx, event) {");
            tb.add("            let state = self.animator.swap_out_state();");
            tb.add("            self.apply(cx, ApplyFrom::Animate, 0, &state);");
            tb.add("            self.animator.swap_in_state(state);");
            //            tb.add("            cx.redraw_child_area(self.bg.draw_call_vars.area);");
            tb.add("        }");
            tb.add("    }");
            tb.add("}");
            
            return tb.end();
        }
    }
    else if parser.eat_ident("enum") {
        if let Some(enum_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(None);
            tb.add("impl").stream(generic.clone());
            tb.add("LiveComponentHooks for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{}");
            return tb.end();
        }
    }
    return parser.unexpected()
}

pub fn derive_live_component_hooks_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(None); //Some("LiveUpdateHooks"));
            
            let fields = if let Some(_types) = types {
                return parser.unexpected();
            }
            else if let Some(fields) = parser.eat_all_struct_fields() {
                fields
            }
            else {
                return parser.unexpected();
            };
            
            let deref_target = fields.iter().find( | field | field.name == "deref_target");
            let draw_call_vars = fields.iter().find( | field | field.name == "draw_call_vars");
            let animator = fields.iter().find( | field | field.name == "animator");
            
            if deref_target.is_some() && draw_call_vars.is_some() ||
            deref_target.is_some() && animator.is_some() ||
            animator.is_some() && draw_call_vars.is_some() {
                // no can do
                return error("Cannot generate LiveComponentHooks with more than one of: deref_target/draw_call_vars/animator");
            }
            
            if let Some(_) = draw_call_vars {
                tb.add("impl").stream(generic.clone());
                tb.add("LiveComponentHooks for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
                tb.add("    fn apply_value_unknown(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index:usize, nodes:&[LiveNode]) -> usize {");
                tb.add("        self.draw_call_vars.apply_value(cx, apply_from, index, nodes)");
                tb.add("    }");
                tb.add("    fn before_apply(&mut self, cx:&mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode]){");
                tb.add("        self.draw_call_vars.before_apply(cx, apply_from, index, nodes, &self.geometry);");
                tb.add("    }");
                tb.add("    fn after_apply(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode]) {");
                tb.add("        self.draw_call_vars.after_apply(cx, apply_from, index, nodes);");
                tb.add("    }");
                tb.add("}");
            }
            else if let Some(_) = deref_target {
                tb.add("impl").stream(generic.clone());
                tb.add("LiveComponentHooks for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
                tb.add("    fn apply_value_unknown(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index:usize, nodes:&[LiveNode]) -> usize{");
                tb.add("        self.deref_target.apply_value_unknown(cx, apply_from, index, nodes)");
                tb.add("    }");
                tb.add("    fn before_apply(&mut self, cx:&mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode]){");
                tb.add("        self.deref_target.before_apply(cx, apply_from, index, nodes);");
                tb.add("    }");
                tb.add("    fn after_apply(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode]) {");
                tb.add("        self.deref_target.after_apply(cx, apply_from, index, nodes);");
                tb.add("    }");
                tb.add("}");
            }
            else if let Some(_) = animator {
                tb.add("impl").stream(generic.clone());
                tb.add("LiveComponentHooks for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
                tb.add("    fn after_apply(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode]) {");
                tb.add("        if let Some(file_id) = apply_from.file_id() {");
                tb.add("            self.animator.live_ptr = Some(LivePtr::from_index(file_id, index));");
                tb.add("            if let Ok(index) = nodes.child_by_name(index, id!(state_default)) {");
                tb.add("                self.apply(cx, ApplyFrom::Animate, index, nodes);");
                tb.add("            }");
                tb.add("        }");
                tb.add("    }");
                tb.add("}");
            }
            else {
                tb.add("impl").stream(generic.clone());
                tb.add("LiveComponentHooks for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{}");
            }
            
            return tb.end();
        }
    }
    else if parser.eat_ident("enum") {
        if let Some(enum_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(None);
            tb.add("impl").stream(generic.clone());
            tb.add("LiveComponentHooks for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{}");
            return tb.end();
        }
    }
    return parser.unexpected()
}

pub fn derive_live_component_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(None); //Some("LiveUpdateHooks"));
            
            let fields = if let Some(_types) = types {
                return parser.unexpected();
            }
            else if let Some(fields) = parser.eat_all_struct_fields() {
                fields
            }
            else {
                return parser.unexpected();
            };
            
            let deref_target = fields.iter().find( | field | field.name == "deref_target");
            
            // alright now. we have a field
            for field in &fields {
                if field.attrs.len() != 1 || field.attrs[0].name != "live" && field.attrs[0].name != "local" && field.attrs[0].name != "hidden" {
                    return error(&format!("Field {} does not have a live, local or hidden attribute", field.name));
                }
            }
            
            if let Some(deref_target) = deref_target {
                tb.add("impl").stream(generic.clone());
                tb.add("std::ops::Deref for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
                tb.add("    type Target = ").stream(Some(deref_target.ty.clone())).add(";");
                tb.add("    fn deref(&self) -> &Self::Target {&self.deref_target}");
                tb.add("}");
                tb.add("impl").stream(generic.clone());
                
                tb.add("std::ops::DerefMut for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
                tb.add("    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.deref_target}");
                tb.add("}");
            }
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveComponentValue for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            
            tb.add("    fn apply_value(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index:usize, nodes:&[LiveNode]) -> usize{");
            tb.add("        match nodes[index].id {");
            for field in &fields {
                if field.attrs[0].name == "live" {
                    tb.add("    Id(").suf_u64(Id::from_str(&field.name).unwrap().0).add(")=>self.").ident(&field.name).add(".apply(cx, apply_from, index, nodes),");
                }
            }
            if deref_target.is_some() {
                tb.add("        _=> self.deref_target.apply_value(cx, apply_from, index, nodes)");
            }
            else {
                tb.add("        _=> self.apply_value_unknown(cx, apply_from, index, nodes)");
            }
            tb.add("        }");
            tb.add("    }");
            
            tb.add("}");
            
            
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveComponent for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn apply(&mut self, cx: &mut Cx, apply_from:ApplyFrom, start_index: usize, nodes: &[LiveNode])->usize {");
            //tb.add("       println!(\"{}\", nodes.to_string(start_index,1));");
            tb.add("        self.before_apply(cx, apply_from, start_index, nodes);");
            tb.add("        let mut index = start_index + 1;"); // skip the class
            tb.add("        loop{");
            tb.add("            if nodes[index].value.is_close(){");
            tb.add("                index += 1;");
            tb.add("                break;");
            tb.add("            }");
            tb.add("            index = self.apply_value(cx, apply_from, index, nodes);");
            tb.add("        }");
            tb.add("        self.after_apply(cx, apply_from, start_index, nodes);");
            tb.add("        return index;");
            tb.add("    }");
            tb.add("}");
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveNew for").ident(&struct_name).stream(generic).stream(where_clause).add("{");
            tb.add("    fn live_type() -> LiveType {");
            tb.add("        LiveType(std::any::TypeId::of::<").ident(&struct_name).add(">())");
            tb.add("    }");
            
            tb.add("    fn live_register(cx: &mut Cx) {");
            tb.add("        struct Factory();");
            tb.add("        impl LiveFactory for Factory {");
            
            tb.add("            fn new_component(&self, cx: &mut Cx) -> Box<dyn LiveComponent> {");
            tb.add("                Box::new(").ident(&struct_name).add(" ::new(cx))");
            tb.add("            }");
            
            tb.add("            fn component_fields(&self, fields: &mut Vec<LiveField>) {");
            for field in &fields {
                let attr = &field.attrs[0];
                if attr.name == "live" || attr.name == "local" {
                    tb.add("fields.push(LiveField{id:Id::from_str(").string(&field.name).add(").unwrap(),");
                    tb.add("live_type:Some(").stream(Some(field.ty.clone())).add("::live_type()),");
                    if attr.name == "live" {
                        tb.add("live_or_local: LiveOrLocal::Live");
                    }
                    else {
                        tb.add("live_or_local: LiveOrLocal::Local");
                    }
                    tb.add("});");
                }
            }
            tb.add("            }");
            
            tb.add("        }");
            tb.add("        cx.register_factory(").ident(&struct_name).add("::live_type(), Box::new(Factory()));");
            tb.add("    }");
            
            tb.add("    fn new_apply(cx: &mut Cx, apply_from:ApplyFrom, index:usize, nodes:&[LiveNode]) -> Self {");
            tb.add("        let mut ret = Self::new(cx);");
            tb.add("        ret.apply(cx, apply_from, index, nodes);");
            tb.add("        ret");
            tb.add("    }");
            
            tb.add("    fn new_from_doc(cx: &mut Cx, live_doc_nodes:LiveDocNodes) -> Self {");
            tb.add("        let mut ret = Self::new(cx);");
            tb.add("        ret.apply(cx, ApplyFrom::NewFromDoc{file_id:live_doc_nodes.file_id}, live_doc_nodes.index, live_doc_nodes.nodes);");
            tb.add("        ret");
            tb.add("    }");
            
            tb.add("    fn new(cx: &mut Cx) -> Self {");
            tb.add("        let mut ret = Self {");
            for field in &fields {
                let attr = &field.attrs[0];
                tb.ident(&field.name).add(":");
                if attr.args.is_none () || attr.args.as_ref().unwrap().is_empty() {
                    if attr.name == "live" {
                        tb.add("LiveNew::new(cx)");
                    }
                    else {
                        tb.add("Default::default()");
                    }
                }
                else {
                    tb.stream(attr.args.clone());
                }
                tb.add(",");
            }
            tb.add("        };");
            tb.add("        ret.after_new(cx);ret");
            tb.add("    }");
            tb.add("}");
            
            return tb.end();
        }
    }
    
    else if parser.eat_ident("enum") {
        if let Some(enum_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(None);
            
            if !parser.open_brace() {
                return parser.unexpected()
            }
            
            struct EnumItem {
                name: String,
                attributes: Vec<Attribute>,
                kind: EnumKind
            }
            
            enum EnumKind {
                Bare,
                Named(Vec<StructField>),
                Tuple(Vec<TokenStream>)
            }
            let mut items = Vec::new();
            
            impl EnumItem {
                
                fn gen_new(&self, tb: &mut TokenBuilder) {
                    tb.add("Self::").ident(&self.name);
                    match &self.kind {
                        EnumKind::Bare => (),
                        EnumKind::Named(_) => {tb.add("{").stream(self.attributes[0].args.clone()).add("}");},
                        EnumKind::Tuple(_) => {tb.add("(").stream(self.attributes[0].args.clone()).add(")");}
                    }
                }
            }
            
            let mut default = None;
            while !parser.eat_eot() {
                let attributes = parser.eat_attributes();
                // check if we have a default attribute
                if let Some(name) = parser.eat_any_ident() {
                    if attributes.len() != 1 {
                        return error(&format!("Field {} does not have a live or default attribute", name));
                    }
                    if attributes[0].name == "default" {
                        if default.is_some() {
                            return error(&format!("Enum can only have a single field marked default"));
                        }
                        default = Some(items.len())
                    }
                    if let Some(types) = parser.eat_all_types() {
                        items.push(EnumItem {name, attributes, kind: EnumKind::Tuple(types)})
                    }
                    else if let Some(fields) = parser.eat_all_struct_fields() { // named variant
                        items.push(EnumItem {name, attributes, kind: EnumKind::Named(fields)})
                    }
                    else if parser.is_punct_alone(',') || parser.is_eot() { // bare variant
                        items.push(EnumItem {name, attributes, kind: EnumKind::Bare})
                    }
                    else {
                        return parser.unexpected();
                    }
                }
                //eprintln!("HERE2");
                parser.eat_punct_alone(',');
            }
            
            if default.is_none() {
                return error(&format!("Enum needs atleast one field marked default"));
            }
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveNew for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            
            tb.add("    fn new(cx:&mut Cx) -> Self { let mut ret = ");
            items[default.unwrap()].gen_new(&mut tb);
            tb.add("    ;ret.after_new(cx);ret");
            tb.add("    }");
            tb.add("    fn new_apply(cx: &mut Cx, apply_from: ApplyFrom, index:usize, nodes:&[LiveNode]) -> Self {");
            tb.add("        let mut ret = Self::new(cx);");
            tb.add("        ret.apply(cx, apply_from, index, nodes);");
            tb.add("        ret");
            tb.add("    }");
            tb.add("    fn new_from_doc(cx: &mut Cx, live_doc_nodes:LiveDocNodes) -> Self {");
            tb.add("        let mut ret = Self::new(cx);");
            tb.add("        ret.apply(cx, ApplyFrom::NewFromDoc{file_id:live_doc_nodes.file_id}, live_doc_nodes.index, live_doc_nodes.nodes);");
            tb.add("        ret");
            tb.add("    }");
            
            tb.add("    fn live_type() -> LiveType {");
            tb.add("        LiveType(std::any::TypeId::of::<").ident(&enum_name).add(">())");
            tb.add("    }");
            tb.add("    fn live_register(cx: &mut Cx) {");
            /*
            tb.add("        let base_name = Id(").suf_u64(Id::from_str(&enum_name).unwrap().0).add(");");
            tb.add("        let mut bare = Vec::new();");
            tb.add("        let mut named = Vec::new();");
            tb.add("        let mut tuple = Vec::new();");
            for item in &items{
                match item.kind{
                    EnumKind::Bare => tb.add("bare.push(Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add("));"),
                    EnumKind::Named(_) => tb.add("named.push(Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add("));"),
                    EnumKind::Tuple(_) => tb.add("tuple.push(Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add("));")
                };
            }
            tb.add("        cx.register_enum(").ident(&enum_name).add("::live_type(),LiveEnumInfo{base_name, bare, named, tuple});");
            */
            tb.add("    }");
            tb.add("}");
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveComponent for").ident(&enum_name).stream(generic).stream(where_clause).add("{");
            
            tb.add("    fn apply(&mut self, cx: &mut Cx, apply_from:ApplyFrom, start_index:usize, nodes: &[LiveNode]) -> usize {");
            tb.add("        self.before_apply(cx, apply_from, start_index, nodes);");
            tb.add("        let mut index = start_index;");
            tb.add("        match &nodes[start_index].value{");
            tb.add("            LiveValue::BareEnum{base,variant}=>{");
            tb.add("                match variant{");
            for item in &items {
                if let EnumKind::Bare = item.kind {
                    tb.add("            Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add(")=>{index += 1;*self = Self::").ident(&item.name).add("},");
                }
            }
            tb.add("                    _=>{");
            tb.add("                        println!(").string("Enum {} cannot find id {}").add(", ").string(&enum_name).add(",variant);");
            tb.add("                        index = nodes.skip_node(index);");
            tb.add("                    }");
            tb.add("                }");
            tb.add("            },");
            
            tb.add("            LiveValue::NamedEnum{base, variant}=>{");
            tb.add("                match variant{");
            for item in &items {
                if let EnumKind::Named(fields) = &item.kind {
                    tb.add("            Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add(")=>{");
                    tb.add("                if let Self::").ident(&item.name).add("{..} = self{}");
                    tb.add("                else{*self = ");
                    item.gen_new(&mut tb);
                    tb.add("                }");
                    tb.add("                if let Self::").ident(&item.name).add("{");
                    for field in fields {
                        tb.ident(&field.name).add(":").ident(&format!("prefix_{}", field.name)).add(",");
                    }
                    tb.add("                } = self {");
                    tb.add("                    index += 1;"); // skip the class
                    tb.add("                    loop{");
                    tb.add("                        if nodes[index].value.is_close(){");
                    tb.add("                            index += 1;");
                    tb.add("                            break;");
                    tb.add("                        }");
                    tb.add("                        match nodes[index].id{");
                    for field in fields {
                        tb.add("                        Id(").suf_u64(Id::from_str(&field.name).unwrap().0).add(")=>{index = (*");
                        tb.ident(&format!("prefix_{}", field.name)).add(").apply(cx, apply_from, index, nodes);},");
                    }
                    tb.add("                            _=>{");
                    tb.add("                                println!(").string("Enum {} cannot find named struct {} property {}").add(", ").string(&enum_name).add(",nodes[start_index].id, nodes[index].id);");
                    tb.add("                                index = nodes.skip_node(index);");
                    tb.add("                            }");
                    tb.add("                        }");
                    tb.add("                    }");
                    tb.add("                }");
                    tb.add("            }");
                }
            }
            tb.add("                    _=>{");
            tb.add("                        println!(").string("Enum {} cannot find named struct {}").add(", ").string(&enum_name).add(", nodes[start_index].id);");
            tb.add("                        index = nodes.skip_node(index);");
            tb.add("                    }");
            tb.add("                }");
            tb.add("            }");
            tb.add("            LiveValue::TupleEnum{base, variant}=>{");
            tb.add("                match variant{");
            
            for item in &items {
                if let EnumKind::Tuple(args) = &item.kind {
                    tb.add("            Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add(")=>{");
                    
                    tb.add("                if let Self::").ident(&item.name).add("{..} = self{}");
                    tb.add("                else{*self = ");
                    item.gen_new(&mut tb);
                    tb.add("                }");
                    
                    tb.add("                if let Self::").ident(&item.name).add("(");
                    for i in 0..args.len() {
                        tb.ident(&format!("var{}", i)).add(",");
                    }
                    tb.add("                ) = self{");
                    tb.add("                    index += 1;"); // skip the class
                    tb.add("                    loop{");
                    tb.add("                        if nodes[index].value.is_close(){");
                    tb.add("                            index += 1;");
                    tb.add("                            break;");
                    tb.add("                        }");
                    tb.add("                        let arg = index - start_index - 1;");
                    tb.add("                        match arg{");
                    for i in 0..args.len() {
                        tb.add("                        ").unsuf_usize(i).add("=>{index = (*").ident(&format!("var{}", i)).add(").apply(cx, apply_from, index, nodes); },");
                    }
                    tb.add("                            _=>{");
                    tb.add("                                println!(").string("Enum {} cannot find tuple struct {} arg {}").add(", ").string(&enum_name).add(", nodes[start_index].id, arg);");
                    tb.add("                                index = nodes.skip_node(index);");
                    tb.add("                            }");
                    tb.add("                        }");
                    tb.add("                    }");
                    tb.add("                }");
                    tb.add("            }");
                }
            }
            tb.add("                    _=>{");
            tb.add("                        println!(").string("Enum {} cannot find tuple struct {}").add(", ").string(&enum_name).add(",nodes[start_index].id);");
            tb.add("                        index = nodes.skip_node(index);");
            tb.add("                    }");
            tb.add("                }");
            tb.add("            }");
            tb.add("            _=>{");
            tb.add("               index = nodes.skip_node(index);");
            tb.add("            }");
            tb.add("        }");
            tb.add("        self.after_apply(cx, apply_from, start_index, nodes);");
            tb.add("        index");
            tb.add("    }");
            
            tb.add("}");
            
            //tb.eprint();
            return tb.end();
        }
    }
    return parser.unexpected()
}