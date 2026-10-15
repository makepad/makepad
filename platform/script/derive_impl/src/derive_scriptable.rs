use makepad_micro_proc_macro::proc_macro::TokenStream;

use makepad_micro_proc_macro::{error_result, Attribute, LiveId, StructField, TokenBuilder, TokenParser};

/// The one resolve call of a derived apply: `__slots[i]` gets the value of the i-th of `names`
/// (declaration order) along `value`'s prototype chain (ScriptVm::script_derive_resolve); the id
/// table is sorted here, at expansion time, for the VM's binary search.
fn emit_resolve(tb: &mut TokenBuilder, names: &[&str]) {
    let mut sorted: Vec<(u64, usize, &str)> = names.iter().enumerate().map(|(i, n)| (LiveId::from_str(n).0, i, *n)).collect();
    sorted.sort();
    tb.add("let mut __slots = [ScriptValue::NIL;").unsuf_usize(names.len()).add("];");
    tb.add("vm.script_derive_resolve(apply, value, &[");
    for (_, i, n) in &sorted {
        tb.add("(").live_id(n).add(",").suf_u32(*i as u32).add("),");
    }
    tb.add("], &mut __slots);");
}

/// A field's name in script: a raw identifier (`r#in`, a keyword as a
/// field name) is the keyword itself.
fn script_name(name: &str) -> &str {
    name.strip_prefix("r#").unwrap_or(name)
}

pub fn derive_script_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    if let Err(err) = derive_script_impl_inner(&mut parser, &mut tb) {
        err
    } else {
        tb.end()
    }
}

/// The default a `#[live(expr)]` field declares for itself, if it does.
fn declared_default(field: &StructField) -> Option<TokenStream> {
    field
        .attrs
        .iter()
        .find(|a| a.name == "live")
        .and_then(|a| a.args.clone())
        .filter(|args| !args.is_empty())
}

/// Adds the expression for a field's declared default (`args`), converted to its type.
fn add_declared_default(tb: &mut TokenBuilder, field: &StructField, args: TokenStream) {
    // for primitive numeric fields, cast instead of .into() -
    // unsuffixed literals like #[live(1.0)] on an f32 field
    // otherwise hit the deprecated f64->f32 inference fallback
    let ty = field.ty.to_string().replace(' ', "");
    if matches!(
        ty.as_str(),
        "f32" | "f64"
            | "i8" | "i16" | "i32" | "i64" | "i128" | "isize"
            | "u8" | "u16" | "u32" | "u64" | "u128" | "usize"
    ) {
        tb.add("(")
            .stream(Some(args))
            .add(") as ")
            .stream(Some(field.ty.clone()));
    } else {
        tb.add("(").stream(Some(args)).add(").into()");
    }
}

fn derive_script_impl_inner(
    parser: &mut TokenParser,
    tb: &mut TokenBuilder,
) -> Result<(), TokenStream> {
    let main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        let struct_name = parser.expect_any_ident()?;
        let generic = parser.eat_generic();
        let types = parser.eat_all_types();
        let where_clause = parser.eat_where_clause(None);

        let mut fields = if let Some(_types) = types {
            return error_result("Unexpected type form");
        } else if let Some(fields) = parser.eat_all_struct_fields() {
            fields
        } else {
            return error_result("Unexpected field form");
        };

        for field in &mut fields {
            if field.attrs.is_empty() {
                // need field def
                return error_result("Please annotate the field type with #[rust] for rust-only fields, #[live] for scriptable mapped fields, #[apply_default] for scriptable fields with default application, #[deref] for a base class, #[script] to call script_new, or #[self] for a ScriptObjectRef to the object being applied");
            }
        }

        // Deref

        let deref_field = fields
            .iter()
            .find(|field| field.attrs.iter().any(|a| a.name == "deref"));

        if let Some(deref_field) = deref_field {
            tb.add("impl").stream(generic.clone());
            tb.add("std::ops::Deref for")
                .ident(&struct_name)
                .stream(generic.clone())
                .stream(where_clause.clone())
                .add("{");
            tb.add("    type Target = ")
                .stream(Some(deref_field.ty.clone()))
                .add(";");
            tb.add("    fn deref(&self) -> &Self::Target {&self.")
                .ident(&deref_field.name)
                .add("}");
            tb.add("}");
            tb.add("impl").stream(generic.clone());

            tb.add("std::ops::DerefMut for")
                .ident(&struct_name)
                .stream(generic.clone())
                .stream(where_clause.clone())
                .add("{");
            tb.add("    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.")
                .ident(&deref_field.name)
                .add("}");
            tb.add("}");
        }

        // marker
        tb.add("impl").stream(generic.clone());
        tb.add("ScriptDeriveMarker for")
            .ident(&struct_name)
            .stream(generic.clone())
            .stream(where_clause.clone())
            .add("{}");

        // ScriptApply

        tb.add("impl").stream(generic.clone());
        tb.add("ScriptApply for")
            .ident(&struct_name)
            .stream(generic.clone())
            .stream(where_clause.clone())
            .add("{");
        tb.add("    fn script_type_id(&self)->ScriptTypeId{ <Self as ScriptNew>::script_type_id_static() }");

        tb.add("    fn script_apply(&mut self, vm:&mut ScriptVm, apply:&Apply, scope:&mut Scope, value:ScriptValue) {");
        tb.add("           if <Self as ScriptHook>::on_custom_apply(self, vm, apply, scope, value) || value.is_nil(){return};");
        tb.add("           <Self as ScriptHook>::on_before_apply(self, vm, apply, scope, value);");
        tb.add("           <Self as ScriptHook>::on_before_dispatch(self, vm, apply, scope, value);");

        let ui_root_field = fields.iter().find_map(|field| {
            let ty = field.ty.to_string().replace(' ', "");
            if field.name == "ui" && (ty == "WidgetRef" || ty.ends_with("::WidgetRef")) {
                Some(field.name.clone())
            } else {
                None
            }
        });

        // The `#[live]` / `#[apply_default]` fields, resolved in one VM call; each field's code
        // below applies its slot.
        let resolved: Vec<&str> = fields
            .iter()
            .filter(|field| {
                !field.attrs.iter().any(|a| a.name == "source")
                    && field.attrs.iter().any(|a| a.name == "live" || a.name == "apply_default")
            })
            .map(|field| script_name(&field.name))
            .collect();
        if !resolved.is_empty() {
            emit_resolve(tb, &resolved);
        }
        let mut slot = 0usize;

        for field in &fields {
            // Handle #[source] field specially - only update during New/Reload, NOT during Eval
            // Eval creates temporary prototype objects that shouldn't become the permanent source
            if field.attrs.iter().any(|a| a.name == "source") {
                tb.add("if !apply.is_eval() && apply.is_from_script() {");
                tb.add("    if let Some(obj) = value.as_object() {");
                tb.add("        self.")
                    .ident(&field.name)
                    .add(" = vm.new_object_ref(obj);");
                tb.add("    }");
                tb.add("}");
            } else if field
                .attrs
                .iter()
                .any(|a| a.name == "live" || a.name == "apply_default")
            {
                // Runtime widget state survives a stylesheet reapply and a
                // Rebake (`script_mod` re-run with unchanged DSL). Explicit
                // edits and ordinary source reloads still update the property.
                // `#[apply_state]` is the stylesheet-reapply mark; `#[visible]`
                // / `#[imperative]` mark fields whose canonical mutation path
                // is an imperative setter sharing storage with the DSL value.
                let preserve_state = field.attrs.iter().any(|a| a.name == "apply_state");
                let imperative = field
                    .attrs
                    .iter()
                    .any(|a| a.name == "imperative" || a.name == "visible");
                if preserve_state || imperative {
                    tb.add("if !apply.preserves_runtime_state() {");
                }
                // One call into a helper generic over the field type: it
                // applies the field's resolved slot, or on a reload the
                // type's registered default when the field is set nowhere.
                // A field with a default of its own (`#[live(expr)]`) falls back
                // to that instead, the same as a new one gets; e.g. a Label's
                // `#[live(Flow::right_wrap())] flow` would otherwise stop
                // wrapping after a reload.
                if let Some(args) = declared_default(field) {
                    tb.add("vm.script_derive_apply_slot_or_default(apply, scope, __slots[")
                        .unsuf_usize(slot)
                        .add("], &mut self.")
                        .ident(&field.name)
                        .add(", || ");
                    add_declared_default(tb, field, args);
                    tb.add(");");
                } else {
                    tb.add("vm.script_derive_apply_slot(apply, scope, __slots[")
                        .unsuf_usize(slot)
                        .add("], &mut self.")
                        .ident(&field.name)
                        .add(");");
                }
                slot += 1;
                if preserve_state || imperative {
                    tb.add("}");
                }
            }
            if field
                .attrs
                .iter()
                .any(|a| a.name == "script_splat" || a.name == "walk" || a.name == "layout")
            {
                tb.add("<")
                    .stream(Some(field.ty.clone()))
                    .add(" as ScriptApply>::script_apply(&mut self.")
                    .ident(&field.name)
                    .add(", vm, apply, scope, value);");
            }
        }

        if let Some(field_name) = &ui_root_field {
            tb.add("self.")
                .ident(field_name)
                .add(".register_as_ui_root(vm);");
        }

        // Deref'd fields are applied BEFORE apply_default's recursive apply.
        //
        // Why this order matters: an `#[apply_default]` field (always an
        // `Animator` in practice) returns an apply block describing how the
        // widget's runtime fields should look in its current state — e.g.
        // `{ height: 0 }` for a "hidden" state. The recursive call then
        // re-walks the widget with that block as the value, setting the
        // matching widget fields. If the deref'd field's `script_apply`
        // (which reapplies the widget's *template* defaults via the inner
        // base widget) ran AFTER this, it would overwrite the animator's
        // state-driven values with the template defaults, leaving the
        // widget visually in its template state for the rest of the apply
        // pass. On `Apply::ScriptReapply` that produces the "flicker to
        // defaults" most users notice when a preference change forces a
        // tree-wide re-walk: every animator-driven widget briefly drops
        // back to its template visual until the next event handler patches
        // it up. Running deref first and apply_default's recursive last
        // means the animator's apply block wins, the widget never visibly
        // touches its template default, and `Animator::script_apply_default`
        // can return the *current* state's apply on `ScriptReapply` to
        // restore the runtime visual state in a single pass.
        for field in &fields {
            if field.attrs.iter().any(|a| a.name == "deref") {
                tb.add("<")
                    .stream(Some(field.ty.clone()))
                    .add(" as ScriptApply>::script_apply(&mut self.")
                    .ident(&field.name)
                    .add(", vm, apply, scope, value);");
            }
        }

        for field in &fields {
            if field.attrs.iter().any(|a| a.name == "apply_default") {
                tb.add("    if let Some(default_value) = <")
                    .stream(Some(field.ty.clone()))
                    .add(" as ScriptApplyDefault>::script_apply_default(&mut self.")
                    .ident(&field.name)
                    .add(",vm, apply, scope, value){");
                tb.add("        self.script_apply(vm, &Apply::Default(apply.as_default().map_or(0, |x| x + 1)), scope, default_value);");
                tb.add("    }");
            }
        }

        tb.add("           <Self as ScriptHook>::on_after_apply(self, vm, apply, scope, value);");
        tb.add("           <Self as ScriptHook>::on_after_dispatch(self, vm, apply, scope, value);");
        tb.add("    }");

        tb.add("    fn script_to_value(&self, vm: &mut ScriptVm)->ScriptValue { vm.script_derive_to_value(self) }");

        tb.add("    fn script_to_value_props(&self, vm: &mut ScriptVm, obj:ScriptObject) {");

        // Consecutive `#[live]` / `#[apply_default]` fields are set in one VM call
        // (script_derive_set_props); a deref / walk / layout / splat cascade between them
        // flushes the batch first, so the object's keys keep their insertion order.
        let mut batch: Vec<&StructField> = Vec::new();
        fn flush(tb: &mut TokenBuilder, batch: &mut Vec<&StructField>) {
            if batch.is_empty() {
                return;
            }
            tb.add("{let __values = [");
            for field in batch.iter() {
                tb.add("<").stream(Some(field.ty.clone())).add(" as ScriptApply>::script_to_value(&self.").ident(&field.name).add(", vm),");
            }
            tb.add("]; vm.script_derive_set_props(obj, &[");
            for field in batch.iter() {
                tb.live_id(script_name(&field.name)).add(",");
            }
            tb.add("], &__values);}");
            batch.clear();
        }
        for field in &fields {
            let cascade = field.attrs.iter().any(|a| a.name == "deref" || a.name == "walk" || a.name == "layout" || a.name == "script_splat");
            if cascade {
                flush(tb, &mut batch);
            }
            if field.attrs.iter().find(|a| a.name == "deref").is_some() {
                tb.add("self.")
                    .ident(&field.name)
                    .add(".script_to_value_props(vm, obj);");
            }
            // Also cascade walk/layout/script_splat fields' properties to the object
            if field
                .attrs
                .iter()
                .find(|a| a.name == "walk" || a.name == "layout" || a.name == "script_splat")
                .is_some()
            {
                tb.add("self.")
                    .ident(&field.name)
                    .add(".script_to_value_props(vm, obj);");
            }
            if field.attrs.iter().any(|a| a.name == "live" || a.name == "apply_default") {
                batch.push(field);
            }
        }
        flush(tb, &mut batch);

        tb.add("    }");

        // Generate script_source. A struct with its own #[source] field reports
        // that object directly. Otherwise, if it derefs to a base class (e.g. a
        // custom widget wrapping `#[deref] view: View`), forward to the base's
        // source so that `script_apply_eval!` on the custom widget can resolve
        // `__script_source__` (and thus the proto scope, type names, etc.) just
        // like it would on the bare base. Without this a custom widget's source
        // falls back to ZERO and every eval against it fails to resolve names.
        let source_field = fields
            .iter()
            .find(|field| field.attrs.iter().any(|a| a.name == "source"));
        if let Some(source_field) = source_field {
            tb.add("    fn script_source(&self) -> ScriptObject {self.")
                .ident(&source_field.name)
                .add(".as_object()");
            tb.add("    }");
            tb.add("    fn script_source_heap_key(&self) -> usize {self.")
                .ident(&source_field.name)
                .add(".heap_key()");
            tb.add("    }");
        } else if let Some(deref_field) = deref_field {
            tb.add("    fn script_source(&self) -> ScriptObject {")
                .add("ScriptApply::script_source(&self.")
                .ident(&deref_field.name)
                .add(")");
            tb.add("    }");
            tb.add("    fn script_source_heap_key(&self) -> usize {")
                .add("ScriptApply::script_source_heap_key(&self.")
                .ident(&deref_field.name)
                .add(")");
            tb.add("    }");
        }

        tb.add("}");

        // ScriptNew

        tb.add("impl").stream(generic.clone());
        tb.add("ScriptNew for")
            .ident(&struct_name)
            .stream(generic.clone())
            .stream(where_clause.clone())
            .add("{");
        // the id of a non-generic type is a constant (ScriptTypeId::of_path of its path, the same
        // under every compiler); generic ones keep the trait's default
        if generic.is_none() {
            tb.add("    fn script_type_id_static()->ScriptTypeId{ const ID: ScriptTypeId = ScriptTypeId::of_path(concat!(module_path!(), \"::\", ").string(&struct_name).add(")); ID }");
        }

        // The name goes to the registration as a value (no function of its
        // own per type).
        tb.add("    fn script_proto(vm:&mut ScriptVm)->ScriptValue{ script_proto_inner(vm, Self::script_type_id_static(), script_proto_build_c::<Self>, script_type_check_c::<Self>, Some(")
            .string(&struct_name)
            .add("), false) }");

        tb.add("    fn script_new(vm: &mut ScriptVm) -> Self {");
        tb.add("        Self {");
        for field in &fields {
            tb.ident(&field.name).add(":");

            if let Some(attr) = field.attrs.iter().find(|a| {
                a.name == "new"
                    || a.name == "live"
                    || a.name == "apply_default"
                    || a.name == "deref"
                    || a.name == "rust"
                    || a.name == "source"
                    || a.name == "uid"
            }) {
                if attr.args.is_none() || attr.args.as_ref().unwrap().is_empty() {
                    if attr.name == "live"
                        || attr.name == "apply_default"
                        || attr.name == "new"
                        || attr.name == "deref"
                    {
                        tb.add("ScriptNew::script_new_with_default(vm)");
                    } else if attr.name == "uid" {
                        tb.add("WidgetUid::new()");
                    } else {
                        tb.add("Default::default()");
                    }
                } else {
                    add_declared_default(tb, field, attr.args.clone().unwrap());
                }
            } else {
                tb.add("Default::default()");
            }
            tb.add(",");
        }
        tb.add("        }");
        tb.add("    }");

        tb.add("    fn script_proto_props(vm: &mut ScriptVm, obj:ScriptObject, props:&mut ScriptTypeProps) {");

        for (_, field) in fields.iter().enumerate() {
            // Process deref field - mark rust_instance_start before adding parent fields
            // This marks where the actual Rust instance data begins (excluding config fields above)
            if field.attrs.iter().find(|a| a.name == "deref").is_some() {
                tb.add("props.mark_rust_instance_start();");
                tb.add("<")
                    .stream(Some(field.ty.clone()))
                    .add(" as ScriptNew>::script_proto_props(vm, obj, props);");
            }

            // Process walk and layout fields - cascade their props like deref but without marking rust_instance_start
            if field
                .attrs
                .iter()
                .find(|a| a.name == "walk" || a.name == "layout" || a.name == "script_splat")
                .is_some()
            {
                tb.add("<")
                    .stream(Some(field.ty.clone()))
                    .add(" as ScriptNew>::script_proto_props(vm, obj, props);");
            }

            // Process live and apply_default fields after deref (or when no deref exists)
            if let Some(_attr) = field
                .attrs
                .iter()
                .find(|a| a.name == "live" || a.name == "apply_default")
            {
                tb.add("vm.script_derive_proto_field::<")
                    .stream(Some(field.ty.clone()))
                    .add(">(props,")
                    .string(script_name(&field.name))
                    .add(");");
            }
        }

        tb.add("    }");
        tb.add("}");

        if main_attribs.iter().any(|attr| attr.name == "debug_print") {
            tb.eprint();
        }

        return Ok(());
    } else if parser.eat_ident("enum") {
        let enum_name = parser.expect_any_ident()?;
        let generic = parser.eat_generic();
        let where_clause = parser.eat_where_clause(None);

        if !parser.open_brace() {
            return error_result("cant find open brace for enum");
        }

        struct EnumItem {
            name: String,
            attributes: Vec<Attribute>,
            kind: EnumKind,
            discriminant: Option<TokenStream>, // For repr(u32) enums - can be any expression
        }

        enum EnumKind {
            Bare,
            Named(Vec<StructField>),
            Tuple(Vec<TokenStream>),
        }
        let mut items = Vec::new();

        impl EnumItem {
            fn gen_new(&self, tb: &mut TokenBuilder) -> Result<(), TokenStream> {
                tb.add("Self::").ident(&self.name);
                match &self.kind {
                    EnumKind::Bare => (),
                    EnumKind::Named(_) => {
                        if self.attributes.len() != 1 {
                            return error_result(
                                "For named and typle enums please provide default values",
                            );
                        }
                        tb.add("{").stream(self.attributes[0].args.clone()).add("}");
                    }
                    EnumKind::Tuple(_) => {
                        if self.attributes.len() != 1 {
                            return error_result(
                                "For named and typle enums please provide default values",
                            );
                        }
                        tb.add("(").stream(self.attributes[0].args.clone()).add(")");
                    }
                }
                Ok(())
            }
        }

        let mut pick = None;
        while !parser.eat_eot() {
            let attributes = parser.eat_attributes();
            // check if we have a default attribute
            if let Some(name) = parser.eat_any_ident() {
                if attributes
                    .iter()
                    .any(|a| a.name == "pick" || a.name == "default")
                {
                    if pick.is_some() {
                        return error_result("Enum can only have a single field marked pick");
                    }
                    pick = Some(items.len())
                }
                if let Some(types) = parser.eat_all_types() {
                    items.push(EnumItem {
                        name,
                        attributes,
                        kind: EnumKind::Tuple(types),
                        discriminant: None,
                    });
                    parser.eat_level_or_punct(',');
                } else if let Some(fields) = parser.eat_all_struct_fields() {
                    // named variant
                    items.push(EnumItem {
                        name,
                        attributes,
                        kind: EnumKind::Named(fields),
                        discriminant: None,
                    });
                    parser.eat_level_or_punct(',');
                } else {
                    // Check for discriminant value (= expr) for bare variants
                    let discriminant = if parser.eat_punct_alone('=') {
                        // Capture everything until comma as the discriminant expression
                        // Note: eat_level_or_punct already consumes the comma
                        Some(parser.eat_level_or_punct(','))
                    } else {
                        parser.eat_level_or_punct(',');
                        None
                    };
                    items.push(EnumItem {
                        name,
                        attributes,
                        kind: EnumKind::Bare,
                        discriminant,
                    })
                }
            } else {
                parser.eat_level_or_punct(',');
            }
        }

        if pick.is_none() {
            return error_result("Enum needs atleast one field marked pick");
        }

        // marker

        tb.add("impl").stream(generic.clone());
        tb.add("ScriptDeriveMarker for")
            .ident(&enum_name)
            .stream(generic.clone())
            .stream(where_clause.clone())
            .add("{}");

        // ScriptNew

        tb.add("impl").stream(generic.clone());
        tb.add("ScriptNew for")
            .ident(&enum_name)
            .stream(generic.clone())
            .stream(where_clause.clone())
            .add("{");
        // the id of a non-generic type is a constant (ScriptTypeId::of_path of its path, the same
        // under every compiler); generic ones keep the trait's default
        if generic.is_none() {
            tb.add("    fn script_type_id_static()->ScriptTypeId{ const ID: ScriptTypeId = ScriptTypeId::of_path(concat!(module_path!(), \"::\", ").string(&enum_name).add(")); ID }");
        }

        // Any variant with a discriminant makes it a repr(u32) enum.
        let has_discriminant = items.iter().any(|item| item.discriminant.is_some());
        tb.add("    fn script_proto(vm:&mut ScriptVm)->ScriptValue{ script_proto_inner(vm, Self::script_type_id_static(), script_proto_build_c::<Self>, script_type_check_c::<Self>, Some(")
            .string(&enum_name)
            .add("),")
            .add(if has_discriminant { "true" } else { "false" })
            .add(") }");
        tb.add("    fn script_new(vm:&mut ScriptVm)->Self{");
        tb.add("       ");
        items[pick.unwrap()].gen_new(tb)?;
        tb.add("       ");
        tb.add("    }");

        tb.add("    fn script_default(vm:&mut ScriptVm)->ScriptValue{");
        tb.add("        Self::script_proto(vm);");
        tb.add("        Self::script_new(vm).script_to_value(vm)");
        tb.add("    }");

        // For enums, script_new_with_default should always return the #[pick] variant,
        // not use the type default (which is the enum API object with all variants)
        tb.add("    fn script_new_with_default(vm:&mut ScriptVm)->Self{");
        tb.add("        Self::script_new(vm)");
        tb.add("    }");

        tb.add("    fn script_reload_default(vm:&mut ScriptVm)->ScriptValue{");
        tb.add(
            "        if vm.heap().type_default_for_id(Self::script_type_id_static()).is_some(){",
        );
        tb.add("            Self::script_new_with_default(vm).script_to_value(vm)");
        tb.add("        } else {");
        tb.add("            NIL");
        tb.add("        }");
        tb.add("    }");

        tb.add("    fn script_type_check(heap:&ScriptHeap, value:ScriptValue)->bool{");
        tb.add("        <Self as ScriptHook>::on_type_check(heap, value) || script_derive_enum_type_check(heap, value, &[");
        for item in &items {
            tb.live_id(&item.name).add(",");
        }
        tb.add("])");
        tb.add("    }");

        tb.add("    fn script_proto_build(vm:&mut ScriptVm, _props:&mut ScriptTypeProps)->ScriptValue{");
        tb.add("        let enum_object = vm.new_object();");

        for item in &items {
            match &item.kind {
                EnumKind::Bare => {
                    // The variant object: its root proto is the variant's id,
                    // it records the enum's name for reflection and, for a
                    // repr(u32) enum, the discriminant as f64.
                    tb.add("vm.script_derive_enum_bare_variant(enum_object,")
                        .string(&item.name)
                        .add(", ")
                        .live_id(&item.name)
                        .add(",")
                        .string(&enum_name)
                        .add(",");
                    if let Some(disc) = &item.discriminant {
                        tb.add("Some((").stream(Some(disc.clone())).add(") as f64)");
                    } else {
                        tb.add("None");
                    }
                    tb.add(");");
                }
                EnumKind::Tuple(args) => {
                    for arg in args.iter() {
                        tb.add("<")
                            .stream(Some(arg.clone()))
                            .add(" as ScriptNew>::script_proto(vm);");
                    }
                    tb.add("vm.add_method(enum_object, ")
                        .live_id_lut(&item.name)
                        .add(", &[], |vm, args|{");
                    // A non-generic helper builds the tuple and checks its
                    // arguments with each type's check; `file!()` and
                    // `line!()` name this expansion for its errors, as the
                    // `script_err_*!` calls it replaces did.
                    tb.add("    vm.script_derive_enum_tuple_new(args, ")
                        .live_id(&item.name)
                        .add(",")
                        .string(&enum_name)
                        .add(", &[");
                    for arg in args.iter() {
                        tb.add("script_type_check_c::<")
                            .stream(Some(arg.clone()))
                            .add(">,");
                    }
                    tb.add("], file!(), line!())");
                    tb.add("});");
                }
                EnumKind::Named(fields) => {
                    tb.add("let def =");
                    item.gen_new(tb)?;
                    tb.add(";");
                    tb.add("let named = vm.new_with_proto(")
                        .live_id_lut(&item.name)
                        .add(");");
                    tb.add("let mut props = ScriptTypeProps::default();");
                    tb.add("if let Self::").ident(&item.name).add("{");
                    for (i, field) in fields.iter().enumerate() {
                        tb.ident(&field.name)
                            .add(":")
                            .ident(&format!("v{i}"))
                            .add(",");
                    }
                    tb.add("} = def{");
                    for (i, field) in fields.iter().enumerate() {
                        tb.add("let value = ")
                            .ident(&format!("v{i}"))
                            .add(".script_to_value(vm);");
                        // Register the field type first (like structs do) so type checking works
                        tb.add("<")
                            .stream(Some(field.ty.clone()))
                            .add(" as ScriptNew>::script_proto(vm);");
                        tb.add("props.insert(")
                            .live_id_lut(&field.name)
                            .add(", <")
                            .stream(Some(field.ty.clone()))
                            .add(" as ScriptNew>::script_type_id_static());");
                        tb.add(" vm.set_value_trap(named, ")
                            .live_id(&field.name)
                            .add(", value);");
                    }
                    tb.add("}");
                    tb.add("let ty_check = ScriptTypeCheck::from_props(&mut props);");
                    tb.add("let ty_index = vm.heap_mut().register_type(None, ty_check);");
                    // Set hidden metadata before attaching the restricted property type.
                    tb.add("vm.set_value_trap(named, ").live_id_lut("__enum").add(", ")
                        .live_id_lut(&enum_name)
                        .add(");");
                    tb.add("vm.heap_mut().set_type(named, ty_index);");
                    tb.add("vm.heap_mut().freeze_component(named);");
                    tb.add("vm.set_value_trap(enum_object, ")
                        .live_id(&item.name)
                        .add(", named);");
                    // uh oh crap. we need to get the default value out of the unparsed defaults
                }
            }
        }
        tb.add("    enum_object.into()");
        tb.add("    }");
        tb.add("}");

        // ScriptApply

        tb.add("impl").stream(generic.clone());
        tb.add("ScriptApply for")
            .ident(&enum_name)
            .stream(generic.clone())
            .stream(where_clause.clone())
            .add("{");
        tb.add("    fn script_type_id(&self)->ScriptTypeId{ <Self as ScriptNew>::script_type_id_static() }");

        tb.add("    fn script_apply(&mut self, vm:&mut ScriptVm, apply:&Apply, scope:&mut Scope, value:ScriptValue){");
        tb.add("        if self.on_custom_apply(vm, apply, scope, value){");
        tb.add("            return");
        tb.add("        }");
        tb.add("        if let Some(object) = value.as_object(){");
        tb.add("            let root_proto = vm.heap().root_proto(object);");
        tb.add("            if let Some(id) = root_proto.as_id(){");
        tb.add("                match id{");
        for item in &items {
            match &item.kind {
                EnumKind::Bare => {
                    tb.add("        ").live_id(&item.name).add("=>{");
                    tb.add("            *self = Self::")
                        .ident(&item.name)
                        .add(";");
                    tb.add("            return;");
                    tb.add("        }");
                }
                EnumKind::Tuple(args) => {
                    tb.add("        ").live_id(&item.name).add("=>{");
                    tb.add("            if let Self::")
                        .ident(&item.name)
                        .add("(..)  = self{}else{");
                    tb.add("               *self = ");
                    item.gen_new(tb)?;
                    tb.add(";");
                    tb.add("            }");
                    tb.add("            if let Self::")
                        .ident(&item.name)
                        .add("(");
                    for i in 0..args.len() {
                        tb.ident(&format!("v{i}")).add(",");
                    }
                    tb.add(") = self{");
                    for (i, arg) in args.iter().enumerate() {
                        tb.add(
                            "            if let Some(v) = vm.heap().vec_value_if_exist(object, ",
                        )
                        .unsuf_usize(i)
                        .add("){");
                        tb.add("                 <")
                            .stream(Some(arg.clone()))
                            .add(" as ScriptApply>::script_apply(")
                            .ident(&format!("v{i}"))
                            .add(", vm, apply, scope, v);");
                        tb.add("            }");
                    }
                    tb.add("            }");
                    tb.add("            return;");
                    tb.add("        }");
                }
                EnumKind::Named(fields) => {
                    tb.add("        ").live_id(&item.name).add("=>{");
                    tb.add("            if let Self::")
                        .ident(&item.name)
                        .add("{..}  = self{}else{");
                    tb.add("               *self = ");
                    item.gen_new(tb)?;
                    tb.add(";");
                    tb.add("            }");
                    tb.add("            if let Self::")
                        .ident(&item.name)
                        .add("{");
                    for (i, field) in fields.iter().enumerate() {
                        tb.ident(&field.name)
                            .add(":")
                            .ident(&format!("v{i}"))
                            .add(",");
                    }
                    tb.add("} = self{");
                    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
                    if !names.is_empty() {
                        emit_resolve(tb, &names);
                    }
                    for (i, field) in fields.iter().enumerate() {
                        tb.add("let v = __slots[").unsuf_usize(i).add("];");
                        tb.add("if v != ScriptValue::NOT_FOUND {");
                        tb.add("    <")
                            .stream(Some(field.ty.clone()))
                            .add(" as ScriptApply>::script_apply(")
                            .ident(&format!("v{i}"))
                            .add(", vm, apply, scope, v);");
                        tb.add("}");
                    }
                    tb.add("            }");
                    tb.add("            return;");
                    tb.add("        }");
                }
            }
        }
        // The errors are reported by non-generic helpers; `file!()` and
        // `line!()` stay here so they name this expansion, as the
        // `script_err_unknown_type!` calls they replace did.
        tb.add("                    other=>{");
        tb.add("                        vm.script_derive_enum_unknown_variant(")
            .string(&enum_name)
            .add(", other, object, file!(), line!());");
        tb.add("                        return;");
        tb.add("                    }");
        tb.add("                }");
        tb.add("            }");
        tb.add("            else{");
        tb.add("                vm.script_derive_enum_not_variant(")
            .string(&enum_name)
            .add(", object, file!(), line!());");
        tb.add("                return;");
        tb.add("            }");
        tb.add("        }");
        tb.add("        vm.script_derive_enum_bad_value(")
            .string(&enum_name)
            .add(", value, file!(), line!());");
        tb.add("    }");

        tb.add("    fn script_to_value(&self, vm:&mut ScriptVm)->ScriptValue{");
        tb.add("        if let Some(value) = <Self as ScriptHook>::on_custom_to_value(self, vm) { return value; }");
        tb.add("        match self{");
        for item in &items {
            match &item.kind {
                EnumKind::Bare => {
                    tb.add("Self::").ident(&item.name).add("=>{");
                    tb.add("    Self::script_enum_lookup_variant(vm,")
                        .live_id(&item.name)
                        .add(")");
                    tb.add("}");
                }
                EnumKind::Tuple(args) => {
                    tb.add("Self::").ident(&item.name).add("(");
                    for i in 0..args.len() {
                        tb.ident(&format!("v{i}")).add(",");
                    }
                    tb.add(")=>{");
                    tb.add("    let tuple = vm.new_with_proto(")
                        .live_id(&item.name)
                        .add(");");
                    tb.add("vm.set_value_trap(tuple, ").live_id_lut("__enum").add(", ")
                        .live_id_lut(&enum_name)
                        .add(");");
                    for (i, arg) in args.iter().enumerate() {
                        tb.add("let value = <")
                            .stream(Some(arg.clone()))
                            .add(" as ScriptApply>::script_to_value(")
                            .ident(&format!("v{i}"))
                            .add(",vm);");
                        tb.add("vm.vec_push_trap(tuple, NIL, value);");
                    }
                    tb.add("    tuple.into()");
                    tb.add("}");
                }
                EnumKind::Named(fields) => {
                    tb.add("Self::").ident(&item.name).add("{");
                    for (i, field) in fields.iter().enumerate() {
                        tb.ident(&field.name)
                            .add(":")
                            .ident(&format!("v{i}"))
                            .add(",");
                    }
                    tb.add("}=>{");
                    tb.add("    let proto = Self::script_enum_lookup_variant(vm,")
                        .live_id(&item.name)
                        .add(");");
                    tb.add("    let named = vm.new_with_proto(proto);");
                    for (i, field) in fields.iter().enumerate() {
                        tb.add("let value = <")
                            .stream(Some(field.ty.clone()))
                            .add(" as ScriptApply>::script_to_value(")
                            .ident(&format!("v{i}"))
                            .add(", vm);");
                        tb.add("vm.set_value_trap(named, ")
                            .live_id(&field.name)
                            .add(", value);");
                    }
                    tb.add("    named.into()");
                    tb.add("}");
                }
            }
        }
        tb.add("        }");
        tb.add("    }");

        tb.add("}");

        Ok(())
    } else {
        error_result("Not enum or struct")
    }
}

pub fn derive_script_hook_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let _types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(None); //Some("LiveUpdateHooks"));
            tb.add("impl").stream(generic.clone());
            tb.add("ScriptHook for")
                .ident(&struct_name)
                .stream(generic)
                .stream(where_clause)
                .add("{}");
            return tb.end();
        }
    } else if parser.eat_ident("enum") {
        if let Some(enum_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(None);
            tb.add("impl").stream(generic.clone());
            tb.add("ScriptHook for")
                .ident(&enum_name)
                .stream(generic)
                .stream(where_clause)
                .add("{}");
            return tb.end();
        }
    }
    parser.unexpected()
}
/*

//#[derive(Script)]
#[allow(unused)]
pub enum EnumTest{
    //  #[pick]
    Bare,
    Tuple(f64),
    Named{named_field:f64}
}

impl ScriptHook for EnumTest{
}

impl ScriptNew for EnumTest{
    fn script_type_id_static()->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_new(vm:&mut ScriptVm)->Self{let mut ret = Self::Bare; ret.on_new(vm);ret}

    fn script_default(vm:&mut ScriptVm)->ScriptValue{
        Self::script_proto(vm);
        Self::script_new(vm).script_to_value(vm)
    }

    fn script_type_check(heap:&ScriptHeap, value:ScriptValue)->bool{
        if Self::on_type_check(heap, value){
            return true
        }
        if let Some(o) = value.as_object(){
            let root_proto = heap.root_proto(o);
            if let Some(id) = root_proto.as_id(){
                return match id{
                    id!(Bare)=>true,
                    id!(Tuple)=>true,
                    id!(Named)=>true,
                    _=>false
                }
            }
        }
        false
    }

    fn script_proto_build(vm:&mut ScriptVm, _props:&mut ScriptTypeProps)->ScriptValue{
        let enum_object = vm.bx.heap.new();

        // how do we typecheck an enum type eh
        let bare = vm.bx.heap.new_with_proto(id!(Bare).into());
        vm.bx.heap.set_value(enum_object, id_lut!(Bare).into(), bare.into(), vm.bx.threads.cur().trap.pass());
        vm.bx.heap.freeze(bare);

        // alright next one the tuple
        vm.add_method(enum_object, id!(Tuple), &[], |vm, args|{
            let tuple = vm.bx.heap.new_with_proto(id!(Tuple).into());
            if vm.bx.heap.vec_len(args) != 1 {
                script_err_invalid_args!(vm.bx.threads.cur().trap, "EnumTest::Tuple requires 1 arg");
            }
            if let Some(a) = vm.bx.heap.vec_value_if_exist(args, 0){
                if !f64::script_type_check(&vm.bx.heap, a){
                    script_err_type_mismatch!(vm.bx.threads.cur().trap, "EnumTest::Tuple arg must be f64");
                }
            }
            vm.bx.heap.vec_push_vec(tuple, args, vm.bx.threads.cur().trap.pass());
            tuple.into()
        });

        // we can make a type index prop check for sself thing
        let def = Self::Named{named_field: 1.0};
        let named = vm.bx.heap.new_with_proto(id_lut!(Named).into());
        let mut props = ScriptTypeProps::default();
        if let Self::Named{named_field:v0} = def{

            let value = v0.script_to_value(vm);
            props.insert(id_lut!(named_field), f64::script_type_id_static());
            vm.bx.heap.set_value(named, id!(named_field).into(), value, vm.bx.threads.cur().trap.pass());

        }

        let ty_check = ScriptTypeCheck::from_props(&mut props);
        let ty_index = vm.bx.heap.register_type(None, ty_check);
        vm.bx.heap.freeze_with_type(named, ty_index);
        vm.bx.heap.set_value(enum_object, id!(Named).into(), named.into(), vm.bx.threads.cur().trap.pass());

        enum_object.into()
    }
}

impl ScriptToValue for EnumTest{
    fn script_to_value(&self, vm:&mut ScriptVm)->ScriptValue{
        match self{
            Self::Bare=>{
                Self::script_enum_lookup_variant(vm, id!(Bare))
            }
            Self::Tuple(x)=>{
                let tuple = vm.bx.heap.new_with_proto(id!(Tuple).into());
                let value = x.script_to_value(vm);
                vm.bx.heap.vec_push(tuple, NIL, value, vm.bx.threads.cur().trap.pass());
                tuple.into()
            }
            Self::Named{named_field}=>{
                let proto = Self::script_enum_lookup_variant(vm, id!(Named));
                let named = vm.bx.heap.new_with_proto(proto);
                let value = named_field.script_to_value(vm);
                vm.bx.heap.set_value(named, id_lut!(named_field).into(), value, vm.bx.threads.cur().trap.pass());
                named.into()
            }
        }
    }
}

impl ScriptApply for EnumTest{
    fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_apply(&mut self, vm:&mut ScriptVm, apply:&Apply, value:ScriptValue){
        if self.on_skip_apply(vm, apply, value){
            return
        }
        if let Some(object) = value.as_object(){
            let root_proto = vm.bx.heap.root_proto(object);
            // we now have to fetch the proto LiveId of the object
            if let Some(id) = root_proto.as_id(){
                match id{
                    id!(Bare)=>{
                        *self = Self::Bare;
                        return;
                    }
                    id!(Tuple)=>{
                        if let Self::Tuple(..) = self{} else {*self = Self::Tuple(1.0)};
                        if let Self::Tuple(a1) = self{
                            if let Some(v) = vm.bx.heap.vec_value_if_exist(object, 0){
                                a1.script_apply(vm, apply, v);
                            }
                            return
                        }
                        return
                    }
                    id!(Named)=>{
                        if let Self::Named{..} = self{} else { *self = Self::Named{named_field:1.0}};
                        if let Self::Named{named_field} = self{
                            if let Some(v) = vm.bx.heap.value_apply_if_dirty(value, ScriptValue::from_id(id!(named_field))){
                                named_field.script_apply(vm, apply, v);
                            }
                            return
                        }
                    }
                    _=>{
                    }
                }
            }
        }
        script_err_unknown_type!(vm.bx.threads.cur().trap, "unknown EnumTest variant");
    }
}*/
