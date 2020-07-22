use {
    crate::{
        ast::*,
        generate::{
            BlockGenerator,
            ExprGenerator,
            BackendWriter,
        },
        ident::Ident,
        lit::TyLit,
        ty::Ty,
    },
    std::fmt::Write,
};

pub fn generate_shader(shader: &ShaderAst) -> String {
    let mut string = String::new();
    ShaderGenerator {
        shader,
        string: &mut string,
    }
    .generate_shader();
    string
}

struct ShaderGenerator<'a> {
    shader: &'a ShaderAst,
    string: &'a mut String,
}

impl<'a> ShaderGenerator<'a> {
    fn generate_shader(&mut self) {
        self.generate_fn_decl(self.shader.find_fn_decl(Ident::new("vertex")).unwrap());
        self.generate_fn_decl(self.shader.find_fn_decl(Ident::new("pixel")).unwrap());
    }

    fn generate_fn_decl(&mut self, decl: &FnDecl) {
        for &callee in decl.callees.borrow().as_ref().unwrap().iter() {
            self.generate_fn_decl(self.shader.find_fn_decl(callee).unwrap());
        }
        self.write_ident_and_ty(
            decl.ident,
            decl.return_ty.borrow().as_ref().unwrap(),
        );
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for param in &decl.params {
            write!(self.string, "{}", sep).unwrap();
            self.write_ident_and_ty(
                param.ident,
                param.ty_expr.ty.borrow().as_ref().unwrap(),
            );
            sep = ", ";
        }
        write!(self.string, ") ").unwrap();
        self.generate_block(&decl.block);
        writeln!(self.string).unwrap();
    }

    fn generate_block(&mut self, block: &Block) {
        BlockGenerator {
            shader: self.shader,
            backend_writer: &MetalBackendWriter,
            use_hidden_parameters: true,
            use_generated_constructors: true,
            indent_level: 0,
            string: self.string
        }
        .generate_block(block)
    }

    fn generate_expr(&mut self, expr: &Expr) {
        ExprGenerator {
            shader: self.shader,
            backend_writer: &MetalBackendWriter,
            use_hidden_parameters: true,
            use_generated_constructors: true,
            string: self.string,
        }
        .generate_expr(expr)
    }

    fn write_ident_and_ty(&mut self, ident: Ident, ty: &Ty) {
        MetalBackendWriter.write_ident_and_ty(
            &mut self.string,
            ident,
            ty
        );
    }
}

struct MetalBackendWriter;

impl BackendWriter for MetalBackendWriter {
    fn write_ty_lit(&self, string: &mut String, ty_lit: TyLit) {
        write!(string, "{}", match ty_lit {
            TyLit::Bool => "bool",
            TyLit::Int => "int",
            TyLit::Float => "float",
            TyLit::Bvec2 => "bool2",
            TyLit::Bvec3 => "bool3",
            TyLit::Bvec4 => "bool4",
            TyLit::Ivec2 => "int2",
            TyLit::Ivec3 => "int3",
            TyLit::Ivec4 => "int4",
            TyLit::Vec2 => "float2",
            TyLit::Vec3 => "float3",
            TyLit::Vec4 => "float4",
            TyLit::Mat2 => "mat2",
            TyLit::Mat3 => "mat3",
            TyLit::Mat4 => "mat4",
        }).unwrap();
    }
}