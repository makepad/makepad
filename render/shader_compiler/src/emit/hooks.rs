use crate::emit::{
    write_ident_and_ty, AttributeDeclAttrs, ExprAttrs, ParamAttrs, VaryingDeclAttrs,
};
use crate::ident::Ident;
use crate::ty_lit::TyLit;
use std::collections::HashSet;
use std::fmt::Write;

pub(in crate::emit) fn should_share_decls() -> bool {
    true
}

pub(in crate::emit) fn write_attribute_decls(
    string: &mut String,
    attribute_decls_attrs: &[AttributeDeclAttrs],
) {
    writeln!(string, "struct _mpsc_Attributes {{").unwrap();
    for attribute_decl_attrs in attribute_decls_attrs {
        write!(string, "    ").unwrap();
        write_ident_and_ty(string, attribute_decl_attrs.ident, &attribute_decl_attrs.ty);
        writeln!(string, ";").unwrap();
    }
}

pub(in crate::emit) fn write_varying_decls(
    string: &mut String,
    varying_decls_attrs: &[VaryingDeclAttrs],
) {
    writeln!(string, "struct _mpsc_Varyings {{").unwrap();
    for varying_decl_attrs in varying_decls_attrs {
        write!(string, "    ").unwrap();
        write_ident_and_ty(string, varying_decl_attrs.ident, &varying_decl_attrs.ty);
        writeln!(string, ";").unwrap();
    }
    writeln!(string, "}};").unwrap();
}

pub(in crate::emit) fn write_builtin_ident(string: &mut String, ident: Ident) {
    write!(string, "{}", ident).unwrap()
}

pub(in crate::emit) fn write_fn_ident(string: &mut String, ident: Ident) {
    write!(string, "{}", ident).unwrap()
}

pub(in crate::emit) fn write_params(
    string: &mut String,
    params_attrs: &[ParamAttrs],
    uniform_block_idents: &HashSet<Ident>,
    has_attributes: bool,
    has_input_varyings: bool,
    has_output_varyings: bool,
) {
    let mut sep = "";
    for param_attrs in params_attrs {
        write!(string, "{}{}", sep, param_attrs.string).unwrap();
        sep = ", ";
    }
    for uniform_block_ident in uniform_block_idents {
        write!(
            string,
            "{}_mpsc_{1}_Uniforms _mpsc_{1}_uniforms",
            sep, uniform_block_ident
        )
        .unwrap();
        sep = ", ";
    }
    if has_attributes {
        write!(string, "{}_mpsc_Attributes _mpsc_attributes", sep).unwrap();
        sep = ", ";
    }
    if has_input_varyings {
        write!(string, "{}_mpsc_Varyings _mpsc_varyings", sep).unwrap();
        sep = ", ";
    }
    if has_output_varyings {
        write!(string, "{}out _mpsc_Varyings _mpsc_varyings", sep).unwrap();
    }
}

pub(in crate::emit) fn write_args(
    string: &mut String,
    xs_attrs: &[ExprAttrs],
    uniform_block_idents: &HashSet<Ident>,
    has_attributes: bool,
    has_input_varyings: bool,
    has_output_varyings: bool,
) {
    let mut sep = "";
    for x_attrs in xs_attrs {
        write!(string, "{}{}", sep, x_attrs.value_or_string).unwrap();
        sep = ", ";
    }
    for uniform_block_ident in uniform_block_idents {
        write!(string, "{}_mpsc_{}_uniforms", sep, uniform_block_ident).unwrap();
        sep = ", ";
    }
    if has_attributes {
        write!(string, "{}_mpsc_attributes", sep).unwrap();
        sep = ", ";
    }
    if has_input_varyings || has_output_varyings {
        write!(string, "{}_mpsc_varyings", sep).unwrap();
    }
}

pub(in crate::emit) fn write_attribute_var(string: &mut String, ident: Ident) {
    write!(string, "_mpsc_attributes.{}", ident).unwrap();
}

pub(in crate::emit) fn write_uniform_var(string: &mut String, block_ident: Ident, ident: Ident) {
    write!(string, "_mpsc_{}_uniforms.{}", block_ident, ident).unwrap();
}

pub(in crate::emit) fn write_varying_var(string: &mut String, ident: Ident) {
    write!(string, "_mpsc_varyings.{}", ident).unwrap();
}

pub(in crate::emit) fn write_ty_lit(string: &mut String, ty_lit: TyLit) {
    write!(string, "{}", ty_lit).unwrap();
}
