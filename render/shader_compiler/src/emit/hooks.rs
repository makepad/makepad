use crate::emit::{
    write_ident_and_ty, AttributeDeclAttrs, FnInfo, UniformDeclAttrs,
    VaryingDeclAttrs,
};
use crate::ident::Ident;
use crate::swizzle::Swizzle;
use crate::ty::Ty;
use crate::ty_lit::TyLit;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

pub(in crate::emit) fn should_share_decls() -> bool {
    false
}

pub(in crate::emit) fn write_attribute_decls(
    string: &mut String,
    attribute_decls_attrs: &[AttributeDeclAttrs],
) {
    if !attribute_decls_attrs.is_empty() {
        writeln!(string, "struct _mpsc_Attributes {{").unwrap();
        for attribute_decl_attrs in attribute_decls_attrs {
            write!(string, "    ").unwrap();
            write_ident_and_ty(string, attribute_decl_attrs.ident, &attribute_decl_attrs.ty);
            writeln!(string, ";").unwrap();
        }
        writeln!(string, "}};").unwrap();
    }
    for attribute_decl_attrs in attribute_decls_attrs {
        write!(string, "attribute ").unwrap();
        write_ident_and_ty(string, attribute_decl_attrs.ident, &attribute_decl_attrs.ty);
        writeln!(string, ";").unwrap();
    }
}

pub(in crate::emit) fn write_uniform_decls(
    string: &mut String,
    uniform_decls_attrs_by_block_ident: &HashMap<Ident, Vec<UniformDeclAttrs>>,
) {
    for (block_ident, uniform_decls_attrs) in uniform_decls_attrs_by_block_ident {
        writeln!(string, "struct _mpsc_{}_Uniforms {{", block_ident).unwrap();
        for uniform_decl_attrs in uniform_decls_attrs {
            write!(string, "    ").unwrap();
            write_ident_and_ty(string, uniform_decl_attrs.ident, &uniform_decl_attrs.ty);
            writeln!(string, ";").unwrap();
        }
        writeln!(string, "}};").unwrap();
    }
    for uniform_decls_attrs in uniform_decls_attrs_by_block_ident.values() {
        for uniform_decl_attrs in uniform_decls_attrs {
            write!(string, "uniform ").unwrap();
            write_ident_and_ty(string, uniform_decl_attrs.ident, &uniform_decl_attrs.ty);
            writeln!(string, ";").unwrap();
        }
    }
}

pub(in crate::emit) fn write_varying_decls(
    string: &mut String,
    varying_decls_attrs: &[VaryingDeclAttrs],
) {
    if !varying_decls_attrs.is_empty() {  
        writeln!(string, "struct _mpsc_Varyings {{").unwrap();
        for varying_decl_attrs in varying_decls_attrs {
            write!(string, "    ").unwrap();
            write_ident_and_ty(string, varying_decl_attrs.ident, &varying_decl_attrs.ty);
            writeln!(string, ";").unwrap();
        }
        writeln!(string, "}};").unwrap();
    }
    for varying_decl_attrs in varying_decls_attrs {
        write!(string, "varying ").unwrap();
        write_ident_and_ty(string, varying_decl_attrs.ident, &varying_decl_attrs.ty);
        writeln!(string, ";").unwrap();
    }
}

pub(in crate::emit) fn write_vertex_main(
    string: &mut String,
    attribute_decls_attrs: &[AttributeDeclAttrs],
    uniform_decls_attrs_by_block_ident: &HashMap<Ident, Vec<UniformDeclAttrs>>,
    varying_decls_attrs: &[VaryingDeclAttrs],
    vertex_fn_info: &FnInfo,
) {
    writeln!(string, "fn main() {{").unwrap();

    if !attribute_decls_attrs.is_empty() {
        writeln!(string, "    _mpsc_Attributes _mpsc_attributes;").unwrap();
    }
    let mut size = 0;
    for attribute_decl_attrs in attribute_decls_attrs {
        size += attribute_decl_attrs.ty.size().unwrap();
    }
    let mut unpacker = VarUnpacker {
        string,
        size,
        dst_prefix: "_mpsc_attribute",
        dst_index: 0,
        dst_size: size.min(4),
        dst_offset: 0,        
    };
    for attribute_decl_attr in attribute_decls_attrs {
        unpacker.write(&format!("_mpsc_attributes.{}", attribute_decl_attr.ident), &attribute_decl_attr.ty);
    }

    for (block_ident, uniform_decls_attrs) in uniform_decls_attrs_by_block_ident {
        writeln!(string, "    _mpsc_{0}_Uniforms _mpsc_{0}_uniforms;", block_ident).unwrap();
        for uniform_decl_attrs in uniform_decls_attrs {
            writeln!(string, "    _mpsc_{0}_uniforms.{1} = {1};", block_ident, uniform_decl_attrs.ident).unwrap();
        }
    }

    if !varying_decls_attrs.is_empty() {
        writeln!(string, "    _mpsc_Varyings _mpsc_varyings;").unwrap();
    }

    write!(string, "    gl_Position = vertex(").unwrap();
    let mut sep = "";
    for uniform_block_ident in &vertex_fn_info.deps.uniform_block_idents {
        write!(string, "{}_mpsc_{}_uniforms", sep, uniform_block_ident).unwrap();
        sep = ", ";
    }
    if vertex_fn_info.deps.has_attributes {
        write!(string, "{}_mpsc_attributes", sep).unwrap();
        sep = ", ";
    }
    if vertex_fn_info.deps.has_output_varyings {
        write!(string, "{}_mpsc_varyings", sep).unwrap();
    }
    writeln!(string, ");").unwrap();

    let mut size = 0;
    for varying_decl_attrs in varying_decls_attrs {
        size += varying_decl_attrs.ty.size().unwrap();
    }
    let mut packer = VarPacker {
        string,
        size,
        dst_prefix: "_mpsc_varying",
        dst_index: 0,
        dst_size: size.min(4),
        dst_offset: 0,        
    };
    for varying_decl_attr in varying_decls_attrs {
        packer.write(&format!("_mpsc_varyings.{}", varying_decl_attr.ident), &varying_decl_attr.ty);
    }

    writeln!(string, "}}").unwrap();
}

pub(in crate::emit) fn write_builtin_ident(string: &mut String, ident: Ident) {
    write!(string, "{}", ident).unwrap()
}

pub(in crate::emit) fn write_fn_ident(string: &mut String, ident: Ident) {
    write!(string, "{}", ident).unwrap()
}

pub(in crate::emit) fn write_hidden_params(
    string: &mut String,
    mut sep: &str,
    uniform_block_idents: &HashSet<Ident>,
    has_attributes: bool,
    has_input_varyings: bool,
    has_output_varyings: bool,
) {
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

pub(in crate::emit) fn write_hidden_args(
    string: &mut String,
    mut sep: &str,
    uniform_block_idents: &HashSet<Ident>,
    has_attributes: bool,
    has_input_varyings: bool,
    has_output_varyings: bool,
) {
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

struct VarPacker<'a> {
    size: usize,
    dst_prefix: &'a str,
    dst_index: usize,
    dst_size: usize,
    dst_offset: usize,
    string: &'a mut String,
}

impl<'a> VarPacker<'a> {
    fn write(&mut self, path: &str, ty: &Ty) {
        match ty {
            Ty::Float => {
                self.write_simple(path, 1);
            }
            Ty::Vec2 | Ty::Vec3 | Ty::Vec4 => {
                let src_size = ty.size().unwrap();
                let mut src_offset = 0;
                while src_offset < src_size {
                    let size = (self.dst_size - self.dst_offset).min(src_size - src_offset);
                    self.write_simple(
                        &format!("{}.{}", path, Swizzle::from_range(src_offset..src_offset + size)),
                        size
                    );
                    src_offset += size;
                }
            }
            Ty::Mat2 | Ty::Mat3 | Ty::Mat4 => {
                for index in 0..ty.len().unwrap() {
                    self.write(&format!("{}[{}]", path, index), &ty.elem_ty().unwrap());
                }
            }
            _ => panic!()
        }
    }

    fn write_simple(&mut self, path: &str, size: usize) {
        write!(
            self.string,
            "    {}_{}",
            self.dst_prefix,
            self.dst_index
        ).unwrap();
        if self.dst_size != 1 {
            write!(
                self.string,
                ".{}",
                Swizzle::from_range(self.dst_offset..self.dst_offset + size),
            ).unwrap();
        }
        writeln!(self.string, " = {};", path).unwrap();
        self.dst_offset += size;
        if self.dst_offset == self.dst_size {
            self.size -= size;
            self.dst_index += 1;
            self.dst_size = self.size.min(4);
            self.dst_offset = 0;
        }
    }
}

struct VarUnpacker<'a> {
    size: usize,
    dst_prefix: &'a str,
    dst_index: usize,
    dst_size: usize,
    dst_offset: usize,
    string: &'a mut String,
}

impl<'a> VarUnpacker<'a> {
    fn write(&mut self, path: &str, ty: &Ty) {
        match ty {
            Ty::Float => {
                self.write_simple(path, 1);
            }
            Ty::Vec2 | Ty::Vec3 | Ty::Vec4 => {
                let src_size = ty.size().unwrap();
                let mut src_offset = 0;
                while src_offset < src_size {
                    let size = (self.dst_size - self.dst_offset).min(src_size - src_offset);
                    self.write_simple(
                        &format!("{}.{}", path, Swizzle::from_range(src_offset..src_offset + size)),
                        size
                    );
                    src_offset += size;
                }
            }
            Ty::Mat2 | Ty::Mat3 | Ty::Mat4 => {
                for index in 0..ty.len().unwrap() {
                    self.write(&format!("{}[{}]", path, index), &ty.elem_ty().unwrap());
                }
            }
            _ => panic!()
        }
    }

    fn write_simple(&mut self, path: &str, size: usize) {
        write!(
            self.string,
            "    {} = {}_{}",
            path,
            self.dst_prefix,
            self.dst_index
            ).unwrap();
        if self.dst_size != 1 {
            write!(
                self.string,
                ".{}",
                Swizzle::from_range(self.dst_offset..self.dst_offset + size),
            ).unwrap();
        }
        writeln!(self.string, ";").unwrap();
        self.dst_offset += size;
        if self.dst_offset == self.dst_size {
            self.size -= size;
            self.dst_index += 1;
            self.dst_size = self.size.min(4);
            self.dst_offset = 0;
        }
    }
}