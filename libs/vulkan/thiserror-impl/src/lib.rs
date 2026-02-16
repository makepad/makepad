extern crate proc_macro;

use makepad_micro_proc_macro::{error, Attribute, TokenBuilder, TokenParser};
use proc_macro::{Literal, TokenStream, TokenTree};

#[derive(Clone, Default)]
struct ErrorAttr {
    message: Option<TokenStream>,
    transparent: bool,
}

#[derive(Clone)]
struct FieldSpec {
    name: Option<String>,
    ty: TokenStream,
    from: bool,
    source: bool,
}

#[derive(Clone)]
enum Shape {
    Unit,
    Tuple(Vec<FieldSpec>),
    Named(Vec<FieldSpec>),
}

#[derive(Clone)]
struct VariantSpec {
    name: String,
    error: ErrorAttr,
    shape: Shape,
}

struct StructSpec {
    name: String,
    error: ErrorAttr,
    shape: Shape,
}

struct EnumSpec {
    name: String,
    variants: Vec<VariantSpec>,
}

enum DeriveSpec {
    Struct(StructSpec),
    Enum(EnumSpec),
}

#[proc_macro_derive(Error, attributes(backtrace, error, from, source))]
pub fn derive_error(input: TokenStream) -> TokenStream {
    match parse_and_expand(input) {
        Ok(ts) => ts,
        Err(err) => err,
    }
}

fn parse_and_expand(input: TokenStream) -> Result<TokenStream, TokenStream> {
    let mut parser = TokenParser::new(input);
    let item_attrs = parser.eat_attributes();
    eat_visibility(&mut parser);

    if parser.eat_ident("struct") {
        let spec = parse_struct(&mut parser, item_attrs)?;
        Ok(expand_struct(&spec))
    } else if parser.eat_ident("enum") {
        let spec = parse_enum(&mut parser)?;
        Ok(expand_enum(&spec))
    } else {
        Err(error(
            "thiserror cleanroom: expected `struct` or `enum` after #[derive(Error)]",
        ))
    }
}

fn eat_visibility(parser: &mut TokenParser) {
    if parser.eat_ident("pub") && parser.open_paren() {
        let _ = parser.eat_level();
    }
}

fn parse_struct(
    parser: &mut TokenParser,
    item_attrs: Vec<Attribute>,
) -> Result<StructSpec, TokenStream> {
    let Some(name) = parser.eat_any_ident() else {
        return Err(error("thiserror cleanroom: expected struct name"));
    };
    let generic = parser.eat_generic();
    if generic.is_some() {
        return Err(error(
            "thiserror cleanroom: generic error types are not supported",
        ));
    }

    let mut where_clause = parser.eat_where_clause(None);
    let shape = if parser.open_paren() {
        let fields = parse_tuple_fields(parser)?;
        parser.eat_punct_alone(';');
        Shape::Tuple(fields)
    } else if parser.open_brace() {
        Shape::Named(parse_named_fields(parser)?)
    } else {
        parser.eat_punct_alone(';');
        Shape::Unit
    };
    if where_clause.is_none() {
        where_clause = parser.eat_where_clause(None);
    }
    if where_clause.is_some() {
        return Err(error(
            "thiserror cleanroom: where clauses are not supported",
        ));
    }

    Ok(StructSpec {
        name,
        error: parse_error_attr(&item_attrs),
        shape,
    })
}

fn parse_enum(parser: &mut TokenParser) -> Result<EnumSpec, TokenStream> {
    let Some(name) = parser.eat_any_ident() else {
        return Err(error("thiserror cleanroom: expected enum name"));
    };
    let generic = parser.eat_generic();
    if generic.is_some() {
        return Err(error(
            "thiserror cleanroom: generic error types are not supported",
        ));
    }

    let where_clause = parser.eat_where_clause(None);
    if where_clause.is_some() {
        return Err(error(
            "thiserror cleanroom: where clauses are not supported",
        ));
    }

    if !parser.open_brace() {
        return Err(error("thiserror cleanroom: expected enum body"));
    }

    let mut variants = Vec::new();
    while !parser.eat_eot() {
        let variant_attrs = parser.eat_attributes();
        if parser.eat_eot() {
            break;
        }
        let Some(name) = parser.eat_any_ident() else {
            return Err(error("thiserror cleanroom: expected enum variant name"));
        };

        let shape = if parser.open_paren() {
            Shape::Tuple(parse_tuple_fields(parser)?)
        } else if parser.open_brace() {
            Shape::Named(parse_named_fields(parser)?)
        } else {
            Shape::Unit
        };

        // We don't expect discriminants in naga errors, but consume them to avoid parser stalls.
        if parser.eat_punct_alone('=') {
            let _ = parser.eat_level_or_punct(',');
        } else {
            parser.eat_punct_alone(',');
        }

        variants.push(VariantSpec {
            name,
            error: parse_error_attr(&variant_attrs),
            shape,
        });
    }

    Ok(EnumSpec { name, variants })
}

fn parse_named_fields(parser: &mut TokenParser) -> Result<Vec<FieldSpec>, TokenStream> {
    let mut fields = Vec::new();
    while !parser.eat_eot() {
        let attrs = parser.eat_attributes();
        eat_field_visibility(parser);
        let Some(name) = parser.eat_any_ident() else {
            return Err(error("thiserror cleanroom: expected named field"));
        };
        if !parser.eat_punct_alone(':') {
            return Err(error("thiserror cleanroom: expected named field"));
        }
        let Some(ty) = parse_field_type(parser) else {
            return Err(error("thiserror cleanroom: expected named field type"));
        };
        parser.eat_punct_alone(',');
        fields.push(field_spec(Some(name), ty, &attrs));
    }
    Ok(fields)
}

fn parse_tuple_fields(parser: &mut TokenParser) -> Result<Vec<FieldSpec>, TokenStream> {
    let mut fields = Vec::new();
    while !parser.eat_eot() {
        let attrs = parser.eat_attributes();
        eat_field_visibility(parser);
        let Some(ty) = parse_field_type(parser) else {
            return Err(error("thiserror cleanroom: expected tuple field type"));
        };
        parser.eat_punct_alone(',');
        fields.push(field_spec(None, ty, &attrs));
    }
    Ok(fields)
}

fn eat_field_visibility(parser: &mut TokenParser) {
    if parser.eat_ident("pub") && parser.open_paren() {
        let _ = parser.eat_level();
    }
}

fn parse_field_type(parser: &mut TokenParser) -> Option<TokenStream> {
    let mut tb = TokenBuilder::new();
    let mut saw_any = false;
    while !parser.is_eot() && !parser.is_punct_alone(',') {
        let Some(current) = parser.current.clone() else {
            break;
        };
        tb.extend(current);
        parser.advance();
        saw_any = true;
    }
    if saw_any { Some(tb.end()) } else { None }
}

fn field_spec(name: Option<String>, ty: TokenStream, attrs: &[Attribute]) -> FieldSpec {
    let mut from = false;
    let mut source = false;
    for attr in attrs {
        if attr.name == "from" {
            from = true;
            source = true;
        } else if attr.name == "source" {
            source = true;
        }
    }
    if matches!(name.as_deref(), Some("source")) {
        source = true;
    }
    FieldSpec {
        name,
        ty,
        from,
        source,
    }
}

fn parse_error_attr(attrs: &[Attribute]) -> ErrorAttr {
    let mut out = ErrorAttr::default();
    for attr in attrs {
        if attr.name != "error" {
            continue;
        }
        if let Some(args) = &attr.args {
            let mut parser = TokenParser::new(args.clone());
            if parser.eat_ident("transparent") {
                out.transparent = true;
                continue;
            }
            if let Some(lit) = parser.eat_literal() {
                out.message = Some(literal_stream(lit));
            }
        }
    }
    out
}

fn literal_stream(lit: Literal) -> TokenStream {
    let mut ts = TokenStream::new();
    ts.extend([TokenTree::Literal(lit)]);
    ts
}

fn expand_struct(spec: &StructSpec) -> TokenStream {
    let mut tb = TokenBuilder::new();
    emit_display_impl(&mut tb, DeriveSpec::Struct(StructSpec {
        name: spec.name.clone(),
        error: spec.error.clone(),
        shape: spec.shape.clone(),
    }));
    emit_error_impl(&mut tb, DeriveSpec::Struct(StructSpec {
        name: spec.name.clone(),
        error: spec.error.clone(),
        shape: spec.shape.clone(),
    }));
    emit_from_impls_for_struct(&mut tb, spec);
    tb.end()
}

fn expand_enum(spec: &EnumSpec) -> TokenStream {
    let mut tb = TokenBuilder::new();
    emit_display_impl(
        &mut tb,
        DeriveSpec::Enum(EnumSpec {
            name: spec.name.clone(),
            variants: spec.variants.clone(),
        }),
    );
    emit_error_impl(
        &mut tb,
        DeriveSpec::Enum(EnumSpec {
            name: spec.name.clone(),
            variants: spec.variants.clone(),
        }),
    );
    emit_from_impls_for_enum(&mut tb, spec);
    tb.end()
}

fn emit_display_impl(tb: &mut TokenBuilder, spec: DeriveSpec) {
    match spec {
        DeriveSpec::Struct(spec) => {
            tb.add("impl core::fmt::Display for")
                .ident(&spec.name)
                .add("{ fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {");
            emit_display_body_for_struct(tb, &spec.error, &spec.shape);
            tb.add("} }");
        }
        DeriveSpec::Enum(spec) => {
            tb.add("impl core::fmt::Display for")
                .ident(&spec.name)
                .add("{ fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {");
            tb.add("match self {");
            for variant in &spec.variants {
                emit_variant_pattern(tb, &variant.name, &variant.shape);
                tb.add("=>");
                emit_display_body_for_shape(tb, &variant.error, &variant.shape);
                tb.add(",");
            }
            tb.add("} } }");
        }
    }
}

fn emit_display_body_for_struct(tb: &mut TokenBuilder, error: &ErrorAttr, shape: &Shape) {
    if error.transparent {
        if let Some(source) = pick_source_field_for_struct(shape, true) {
            emit_display_transparent(tb, source);
            return;
        }
    }

    if let Some(message) = &error.message {
        tb.add("{ f.write_str(")
            .stream(Some(message.clone()))
            .add(")?; f.write_str(")
            .string(" | ")
            .add(")?; core::fmt::Debug::fmt(self, f) }");
    } else {
        tb.add("core::fmt::Debug::fmt(self, f)");
    }
}

fn emit_display_body_for_shape(tb: &mut TokenBuilder, error: &ErrorAttr, shape: &Shape) {
    if error.transparent {
        if let Some(source) = pick_source_field_for_variant(shape, true) {
            emit_display_transparent(tb, source);
            return;
        }
    }

    if let Some(message) = &error.message {
        tb.add("{ f.write_str(")
            .stream(Some(message.clone()))
            .add(")?; f.write_str(")
            .string(" | ")
            .add(")?; core::fmt::Debug::fmt(self, f) }");
    } else {
        tb.add("core::fmt::Debug::fmt(self, f)");
    }
}

fn emit_display_transparent(tb: &mut TokenBuilder, source: SourceField) {
    tb.add("core::fmt::Display::fmt(");
    match source {
        SourceField::TupleIndex(i) => {
            tb.ident(&format!("__f{i}"));
        }
        SourceField::Named(name) => {
            tb.ident(&format!("__f_{}", name));
        }
        SourceField::StructTupleIndex(i) => {
            tb.add("&self.").unsuf_usize(i);
        }
        SourceField::StructNamed(name) => {
            tb.add("&self.").ident(&name);
        }
    }
    tb.add(", f)");
}

fn emit_error_impl(tb: &mut TokenBuilder, spec: DeriveSpec) {
    match spec {
        DeriveSpec::Struct(spec) => {
            tb.add("impl core::error::Error for")
                .ident(&spec.name)
                .add("{ fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {");
            if let Some(source) = pick_source_field_for_struct(&spec.shape, spec.error.transparent)
            {
                tb.add("Some(");
                emit_source_ref(tb, source);
                tb.add(")");
            } else {
                tb.add("None");
            }
            tb.add("} }");
        }
        DeriveSpec::Enum(spec) => {
            tb.add("impl core::error::Error for")
                .ident(&spec.name)
                .add("{ fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {");
            tb.add("match self {");
            for variant in &spec.variants {
                emit_variant_pattern(tb, &variant.name, &variant.shape);
                tb.add("=>");
                if let Some(source) =
                    pick_source_field_for_variant(&variant.shape, variant.error.transparent)
                {
                    tb.add("Some(");
                    emit_source_ref(tb, source);
                    tb.add(")");
                } else {
                    tb.add("None");
                }
                tb.add(",");
            }
            tb.add("} } }");
        }
    }
}

fn emit_source_ref(tb: &mut TokenBuilder, source: SourceField) {
    match source {
        SourceField::TupleIndex(i) => {
            tb.ident(&format!("__f{i}"));
        }
        SourceField::Named(name) => {
            tb.ident(&format!("__f_{}", name));
        }
        SourceField::StructTupleIndex(i) => {
            tb.add("&self.").unsuf_usize(i);
        }
        SourceField::StructNamed(name) => {
            tb.add("&self.").ident(&name);
        }
    }
    tb.add(" as &(dyn core::error::Error + 'static)");
}

fn emit_from_impls_for_struct(tb: &mut TokenBuilder, spec: &StructSpec) {
    if let Some((field, source_kind)) = pick_from_field_struct(&spec.shape) {
        tb.add("impl core::convert::From<")
            .stream(Some(field.ty.clone()))
            .add("> for")
            .ident(&spec.name)
            .add("{ fn from(value: ")
            .stream(Some(field.ty.clone()))
            .add(") -> Self {");
        match source_kind {
            SourceKind::StructTuple => {
                tb.ident(&spec.name).add("(value)");
            }
            SourceKind::StructNamed(name) => {
                tb.ident(&spec.name)
                    .add("{")
                    .ident(&name)
                    .add(": value }");
            }
            SourceKind::VariantTuple(_) | SourceKind::VariantNamed(_, _) => {
                tb.ident(&spec.name).add("(value)");
            }
        }
        tb.add("} }");
    }
}

fn emit_from_impls_for_enum(tb: &mut TokenBuilder, spec: &EnumSpec) {
    for variant in &spec.variants {
        let Some((field, source_kind)) = variant.pick_from_field() else {
            continue;
        };
        tb.add("impl core::convert::From<")
            .stream(Some(field.ty.clone()))
            .add("> for")
            .ident(&spec.name)
            .add("{ fn from(value: ")
            .stream(Some(field.ty.clone()))
            .add(") -> Self {");
        match source_kind {
            SourceKind::VariantTuple(variant_name) => {
                tb.ident(&spec.name)
                    .add("::")
                    .ident(&variant_name)
                    .add("(value)");
            }
            SourceKind::VariantNamed(variant_name, field_name) => {
                tb.ident(&spec.name)
                    .add("::")
                    .ident(&variant_name)
                    .add("{")
                    .ident(&field_name)
                    .add(": value }");
            }
            SourceKind::StructTuple | SourceKind::StructNamed(_) => {}
        }
        tb.add("} }");
    }
}

fn emit_variant_pattern(tb: &mut TokenBuilder, variant_name: &str, shape: &Shape) {
    tb.add("Self::").ident(variant_name);
    match shape {
        Shape::Unit => {}
        Shape::Tuple(fields) => {
            tb.add("(");
            for i in 0..fields.len() {
                tb.ident(&format!("__f{i}"));
                if i + 1 != fields.len() {
                    tb.add(",");
                }
            }
            tb.add(")");
        }
        Shape::Named(fields) => {
            tb.add("{");
            for field in fields {
                let Some(name) = &field.name else {
                    continue;
                };
                tb.ident(name)
                    .add(":")
                    .ident(&format!("__f_{}", name))
                    .add(",");
            }
            tb.add("}");
        }
    }
}

#[derive(Clone)]
enum SourceField {
    TupleIndex(usize),
    Named(String),
    StructTupleIndex(usize),
    StructNamed(String),
}

fn pick_source_field_for_variant(shape: &Shape, transparent: bool) -> Option<SourceField> {
    match shape {
        Shape::Unit => None,
        Shape::Tuple(fields) => {
            let flagged = fields
                .iter()
                .enumerate()
                .find_map(|(i, f)| f.source.then_some(i));
            match (flagged, transparent, fields.is_empty()) {
                (Some(i), _, _) => Some(SourceField::TupleIndex(i)),
                (None, true, false) => Some(SourceField::TupleIndex(0)),
                _ => None,
            }
        }
        Shape::Named(fields) => {
            let flagged = fields.iter().find_map(|f| {
                if f.source {
                    f.name.clone()
                } else {
                    None
                }
            });
            match (flagged, transparent, fields.is_empty()) {
                (Some(name), _, _) => Some(SourceField::Named(name)),
                (None, true, false) => fields[0].name.clone().map(SourceField::Named),
                _ => None,
            }
        }
    }
}

fn pick_source_field_for_struct(shape: &Shape, transparent: bool) -> Option<SourceField> {
    match shape {
        Shape::Unit => None,
        Shape::Tuple(fields) => {
            let flagged = fields
                .iter()
                .enumerate()
                .find_map(|(i, f)| f.source.then_some(i));
            match (flagged, transparent, fields.is_empty()) {
                (Some(i), _, _) => Some(SourceField::StructTupleIndex(i)),
                (None, true, false) => Some(SourceField::StructTupleIndex(0)),
                _ => None,
            }
        }
        Shape::Named(fields) => {
            let flagged = fields.iter().find_map(|f| {
                if f.source {
                    f.name.clone()
                } else {
                    None
                }
            });
            match (flagged, transparent, fields.is_empty()) {
                (Some(name), _, _) => Some(SourceField::StructNamed(name)),
                (None, true, false) => fields[0].name.clone().map(SourceField::StructNamed),
                _ => None,
            }
        }
    }
}

#[derive(Clone)]
enum SourceKind {
    StructTuple,
    StructNamed(String),
    VariantTuple(String),
    VariantNamed(String, String),
}

fn pick_from_field_struct(shape: &Shape) -> Option<(&FieldSpec, SourceKind)> {
    match shape {
        Shape::Unit => None,
        Shape::Tuple(fields) => {
            if fields.len() == 1 && fields[0].from {
                Some((&fields[0], SourceKind::StructTuple))
            } else {
                None
            }
        }
        Shape::Named(fields) => {
            if fields.len() == 1 && fields[0].from {
                fields[0]
                    .name
                    .as_ref()
                    .map(|name| (&fields[0], SourceKind::StructNamed(name.clone())))
            } else {
                None
            }
        }
    }
}

impl VariantSpec {
    fn pick_from_field(&self) -> Option<(&FieldSpec, SourceKind)> {
        match &self.shape {
            Shape::Unit => None,
            Shape::Tuple(fields) => {
                if fields.len() == 1 && fields[0].from {
                    Some((&fields[0], SourceKind::VariantTuple(self.name.clone())))
                } else {
                    None
                }
            }
            Shape::Named(fields) => {
                if fields.len() == 1 && fields[0].from {
                    fields[0].name.as_ref().map(|field_name| {
                        (
                            &fields[0],
                            SourceKind::VariantNamed(self.name.clone(), field_name.clone()),
                        )
                    })
                } else {
                    None
                }
            }
        }
    }
}
