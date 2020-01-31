// This proc_macro is used to transform a rust closure function
// of the following form
// shader_ast!(||{
//      // var def:
//      let x:float<Uniform> = 10.0;
//      // fn def:
//      fn pixel()->vec4{
//          return vec4(1.);
//      }
//})
// into a nested tree of shader AST structs
// these are defined in shader.rs in the root project
// which looks something like the following:
// ShAst{
//      vars:vec![ShVar{name:"x".to_string(), ty:"float".to_string()}]   
// }
// The subset of Rust syntax we support is directly related to
// a mapping of GLSL.
// types have to be simple names like float or vec4
// we support for loops only with integer ranges
// think of the subset as how you would write GLSL with a Rust syntax
// not as what you can write in Rust that has no direct
// word for word match in GLSL.

extern crate proc_macro;
extern crate proc_macro2;
use proc_macro_hack::proc_macro_hack;
use proc_macro2::TokenStream;
use proc_macro2::Span;
use syn::{
    Expr, Type, Pat, Stmt, PathArguments, GenericArgument, 
    Item, Local, ItemFn, ItemConst, ItemStruct,
    Lit, Block, FnArg, BinOp, UnOp, Ident, ReturnType, Member
};
use quote::quote;
use quote::quote_spanned;
use syn::spanned::Spanned;

fn error(span:Span, msg: &str)->TokenStream{
    let fmsg = format!("shader_ast: {}", msg);
    quote_spanned!(span=>compile_error!(#fmsg))
}

// generate the ShVar definitions from a let statement
fn generate_shvar_defs(stmt:Local)->TokenStream{
    // lets define a local with storage specified
    if let Pat::Type(pat) = &stmt.pat{
        let name =  if let Pat::Ident(ident) = &*pat.pat{
            ident.ident.to_string()
        }
        else{
            return error(stmt.span(), "Please only use simple identifiers such as x or var_iable");
        };
        let found_type;
        let store;
        if let Type::Path(typath) = &*pat.ty{
            if typath.path.segments.len() != 1{
                return quote!{sh_var(#name, &#typath.shader_type(), #typath.var_store())}
            }

            if typath.path.segments.len() != 1{
                return error(typath.span(), "Only simple typenames such as float or vec4 are supported");
            }
            let seg = &typath.path.segments[0];
            found_type = seg.ident.to_string();
            // lets read the path args
            if let PathArguments::AngleBracketed(angle) = &seg.arguments{
                if angle.args.len() != 1{
                    return error(angle.span(), "Please pass one storage arg like float<Local>");
                }
                let arg = &angle.args[0];
                if let GenericArgument::Type(ty) = arg{
                    if let Type::Path(typath) = ty{
                        if typath.path.segments.len() != 1{
                            return error(typath.span(), "Only simple typenames such as float or vec4 are supported");
                        }
                        let seg = &typath.path.segments[0];
                        store = seg.ident.clone();
                    }
                    else{
                        return error(arg.span(), "Only simple typenames such as float or vec4 are supported");
                    }
                }
                else{
                    return error(arg.span(), "Please pass one storage arg like float<Local>");
                }
            }
            else{
               return error(stmt.span(), "Please pass one storage arg like float<Local>");
            }
        }
        else{
            return error(stmt.span(), "Please give the variable a type of the form float<Local> ");
        }
        return quote!{sh_var(#name, #found_type, ShVarStore::#store)}
    }
    else{
        return error(stmt.span(), "Please only use simple identifiers such as x or var_iable {:?}")
    }
}

// generate the ShFn definitions from a rust fn statement
fn generate_fn_def(item:ItemFn)->TokenStream{
    // alright lets do a function
    // and then incrementally add all supported ast nodes
    let name = item.sig.ident.to_string();
       let mut args = Vec::new();
    // lets process the fnargs
    for arg in &item.sig.inputs{
        if let FnArg::Typed(arg) = arg{
            // lets look at pat and ty
            if let Pat::Ident(pat) = &*arg.pat{
                let name =  pat.ident.to_string();
                let found_type;
                if let Type::Path(typath) = &*arg.ty{
                    if typath.path.segments.len() != 1{
                        return error(typath.span(), "arg type not simple");
                    }
                    let seg = &typath.path.segments[0];
                    found_type = seg.ident.to_string();
                }
                else{
                    return error(arg.span(), "arg type not simple");
                }
                args.push(quote!{sh_fnarg(#name, #found_type)})
            }
            else{
                return error(arg.span(), "arg pattern not simple identifier")
            }
        }
        else{
             return error(arg.span(), "arg pattern not simple identifier")
        }
    }
    let return_type;
    if let ReturnType::Type(_, ty) = item.sig.output{
        if let Type::Path(typath) = *ty{
            if typath.path.segments.len() != 1{
                return error(typath.span(), "return type not simple");
            }
            let seg = &typath.path.segments[0];
            return_type = seg.ident.to_string();
        }
        else{
            return error(ty.span(), "return type not simple");
        }
    }   
    else{
        return_type = "void".to_string();
        //return error(item.span(), "function needs to specify return type")
    }
    let block = generate_block(*item.block);
    quote!{sh_fn(#name, &[#(#args,)*], #return_type, Some(#block))}
}

// generate a let statement inside a function
fn generate_let(local:Local)->TokenStream{
    // lets define a local with storage specified
    if let Pat::Ident(ident) = &local.pat{
        let name = ident.ident.to_string();
        let init = if let Some((_,local_init)) = local.init{
            generate_expr(*local_init)
        }
        else{
            return error(local.span(), "let pattern misses initializer");
        };

        return quote!{sh_let(#name, "", #init)}
    }
    else if let Pat::Type(pat) = &local.pat{
        let name =  if let Pat::Ident(ident) = &*pat.pat{
            ident.ident.to_string()
        }
        else{
            return error(local.span(), "Please only use simple identifiers such as x or var_iable");
        };
        
        let ty = if let Type::Path(typath) = &*pat.ty{
            if typath.path.segments.len() != 1{
                return error(typath.span(), "Only simple typenames such as float or vec4 are supported");
            }
            let seg = &typath.path.segments[0];
            seg.ident.to_string()
        }
        else{
           return error(local.span(), "Only simple typenames such as float or vec4 are supported");
        };

        let init = if let Some((_,local_init)) = local.init{
            generate_expr(*local_init)
        }
        else{
            return error(local.span(), "let pattern misses initializer");
        };
        
        return quote!{sh_let(#name, #ty, #init)}
    }
    else{
        return error(local.span(), "let pattern doesn't need type");
    }
}

// generate a { } block AST 
fn generate_block(block:Block)->TokenStream{
    let mut stmts = Vec::new();
    for stmt in block.stmts{
        match stmt{
            Stmt::Local(stmt)=>{
                let letstmt = generate_let(stmt);
                stmts.push(letstmt)
            }
            Stmt::Item(stmt)=>{
                return error(stmt.span(), "Shader functions don't support items");
            }
            Stmt::Expr(stmt)=>{
                let expr = generate_expr(stmt);
                stmts.push(quote!{sh_exps(#expr)})
            }
            Stmt::Semi(stmt, _tok)=>{
                let expr = generate_expr(stmt);
                stmts.push(quote!{sh_sems(#expr)})
            }
        }
    }
    return quote!{sh_block(&[#(#stmts,)*])}
}

// return the string name of a BinOp enum 
fn get_binop(op:BinOp)->&'static str{
    match op{
        BinOp::Add(_)=>"Add",
        BinOp::Sub(_)=>"Sub",
        BinOp::Mul(_)=>"Mul",
        BinOp::Div(_)=>"Div",
        BinOp::Rem(_)=>"Rem",
        BinOp::And(_)=>"And",
        BinOp::Or(_)=>"Or",
        BinOp::BitXor(_)=>"BitXor",
        BinOp::BitAnd(_)=>"BitAnd",
        BinOp::BitOr(_)=>"BitOr",
        BinOp::Shl(_)=>"Shl",
        BinOp::Shr(_)=>"Shr",
        BinOp::Eq(_)=>"Eq",
        BinOp::Lt(_)=>"Lt",
        BinOp::Le(_)=>"Le",
        BinOp::Ne(_)=>"Ne",
        BinOp::Ge(_)=>"Ge",
        BinOp::Gt(_)=>"Gt",
        BinOp::AddEq(_)=>"AddEq",
        BinOp::SubEq(_)=>"SubEq",
        BinOp::MulEq(_)=>"MulEq",
        BinOp::DivEq(_)=>"DivEq",
        BinOp::RemEq(_)=>"RemEq",
        BinOp::BitXorEq(_)=>"BitXorEq",
        BinOp::BitAndEq(_)=>"BitAndEq",
        BinOp::BitOrEq(_)=>"BitOrEq",
        BinOp::ShlEq(_)=>"ShlEq",
        BinOp::ShrEq(_)=>"ShrEq",
    }
}

// generate the AST from an expression
fn generate_expr(expr:Expr)->TokenStream{
    match expr{
        Expr::Call(expr)=>{
            if let Expr::Path(func) = *expr.func{
                if func.path.segments.len() != 1{
                    return error(func.span(), "call identifier not simple");
                }
                let seg = &func.path.segments[0].ident.to_string();
                // lets get all fn args
                let mut args = Vec::new();
                for arg in expr.args{
                    args.push(generate_expr(arg));
                }
                
                //return quote!{ShExpr::ShCall(ShCall{call:#seg.to_string(), args:{let mut v=Vec::new();#(v.push(#args);)*v}})}
                return quote!{sh_call(#seg, &[#(#args,)*])}
            }
            else{
                 return error(expr.span(), "call identifier not simple");
            }
        }
        Expr::Binary(expr)=>{
            let left = generate_expr(*expr.left);
            let right = generate_expr(*expr.right);
            let op = Ident::new(get_binop(expr.op), Span::call_site());
            return quote!{sh_bin(#left, #right, ShOp::#op)}
        }
        Expr::Unary(expr)=>{
            let op;
            if let UnOp::Not(_) = &expr.op{
                op = Ident::new("Not", Span::call_site());
            }
            else if let UnOp::Neg(_) = &expr.op{
                op = Ident::new("Neg", Span::call_site());
            }
            else {
                return error(expr.span(), "Deref not implemented");
            }
            let right = generate_expr(*expr.expr);
            return quote!{sh_unary(#right, ShUnaryOp::#op)}
        }
        Expr::Lit(expr)=>{
            match expr.lit{
                Lit::Str(lit)=>{
                    let value = lit.value();
                    return quote!{sh_str(#value)}
                }
                Lit::Int(lit)=>{
                    let value = lit.base10_parse::<i64>().unwrap();
                    return quote!{sh_int(#value)}
                }
                Lit::Float(lit)=>{
                    let value = lit.base10_parse::<f64>().unwrap();
                    return quote!{sh_fl(#value)}
                }
                Lit::Bool(lit)=>{
                    let value = lit.value;
                    return quote!{sh_bool(#value)}
                }
                _=>{
                    return error(expr.span(), "Unsupported literal for shader")
                }
            }
        }
        Expr::Let(expr)=>{
            return error(expr.span(), "Not implemented Expr::Let")
        }
        Expr::If(expr)=>{
            let cond = generate_expr(*expr.cond);
            let then_branch = generate_block(expr.then_branch);

            if let Some((_,else_branch)) = expr.else_branch{
                let else_branch = generate_expr(*else_branch);
                return quote!{sh_if_else(#cond, #then_branch, #else_branch)}
            }
            return quote!{sh_if(#cond, #then_branch)}
        }
        Expr::While(expr)=>{
            let cond = generate_expr(*expr.cond);
            let block = generate_block(expr.body);
            return quote!{sh_while(#cond, #block)}
       }
        Expr::ForLoop(expr)=>{
              // lets define a local with storage specified
            let span = expr.span();
            if let Pat::Ident(pat) = expr.pat{
                let name =  pat.ident.to_string();
                let body = generate_block(expr.body);
                let from_ts;
                let to_ts;
                if let Expr::Range(range) = *expr.expr{
                    if let Some(from) = range.from {
                        from_ts = generate_expr(*from);
                    }
                    else{
                        return error(span, "Must provide from range expression")
                    }
                    if let Some(to) = range.to {
                        to_ts = generate_expr(*to);
                    }
                    else{
                        return error(span, "Must provide to range expression")
                    }
                }
                else{
                    return error(span, "Must provide range expression")
                }
                return quote!{sh_for(#name, #from_ts, #to_ts, #body)}
            }
            else{
                return error(expr.span(), "Use simple identifier for for loop")
            }
        }
        Expr::Assign(expr)=>{
            let left = generate_expr(*expr.left);
            let right = generate_expr(*expr.right);
            return quote!{sh_asn(#left, #right)};//ShExpr::ShAssign(ShAssign{left:Box::new(#left),right:Box::new(#right)})}
        }
        Expr::AssignOp(expr)=>{
            let left = generate_expr(*expr.left);
            let right = generate_expr(*expr.right);
            let op = Ident::new(get_binop(expr.op), Span::call_site());
            return quote!{sh_asn_op(#left, #right, ShOp::#op)}
            // return quote!{ShExpr::ShAssignOp(ShAssignOp{left:Box::new(#left),op:ShBinOp::#op,right:Box::new(#right)})}
        }
        Expr::Field(expr)=>{
            let member;
            if let Member::Named(ident) = expr.member{
                member = ident.to_string();
            }
            else{
                return error(expr.span(), "No unnamed members supported")
            }
            let base = generate_expr(*expr.base);
            return quote!{sh_fd(#base, #member)}//ShExpr::ShField(ShField{base:Box::new(#base),member:#member.to_string()})}
        }
        Expr::Index(expr)=>{
            let base = generate_expr(*expr.expr);
            let index = generate_expr(*expr.index);
            return quote!{sh_idx(#base, #index)}//ShExpr::ShIndex(ShIndex{base:Box::new(#base),index:Box::new(#index)})}
        }
        Expr::Path(expr)=>{
            if expr.path.segments.len() != 1{
                return error(expr.span(), "type not simple");
            }
            let seg = &expr.path.segments[0].ident.to_string();
            return quote!{sh_id(#seg)}//ShExpr::ShId(ShId{name:#seg.to_string()})}
        }
        Expr::Paren(expr)=>{
            let expr = generate_expr(*expr.expr);
            return quote!{sh_par(#expr)}//ShExpr::ShParen(ShParen{expr:Box::new(#expr)})}
        }
        Expr::Block(expr)=>{ // process a block expression
            let block = generate_block(expr.block); 
            return quote!{ShExpr::ShBlock(#block)}
        }
        Expr::Return(expr)=>{
            if let Some(expr) = expr.expr{
                let expr = generate_expr(*expr);
                return quote!{sh_ret(#expr)}
            }
            return quote!{sh_retn()}
        }
        Expr::Break(_)=>{
            return quote!{ShExpr::ShBreak(ShBreak{})}

        }
        Expr::Continue(_)=>{
            return quote!{ShExpr::ShContinue(ShContinue{})}
        }
        _=>{
            return error(expr.span(), "Unsupported syntax for shader")
        }
    }
}

// generate the ShConst defs
fn generate_const_def(item:ItemConst)->TokenStream{
    let name = item.ident.to_string();
    let ty;

    if let Type::Path(typath) = *item.ty{
        if typath.path.segments.len() != 1{
            return error(typath.span(), "const type not a basic identifie");
        }
        let seg = &typath.path.segments[0];
        ty = seg.ident.to_string();
    }
    else{
        return error(item.ty.span(), "const type not a basic identifier");
    }

    let expr = generate_expr(*item.expr);
    quote!{
        ShConst{
            name:#name.to_string(),
            ty:#ty.to_string(),
            value:#expr
        }
    }
}

// generate the ShStruct defs
fn generate_struct_def(_item:ItemStruct)->TokenStream{
    TokenStream::new()
}

// Generate the ShAst rootnode
fn generate_root(expr:Expr)->TokenStream{
    let mut vars = Vec::new();
    let mut fns = Vec::new();
    let mut consts = Vec::new();
    let mut structs = Vec::new();
    match expr {
        Expr::Block(expr)=>{
            for stmt in expr.block.stmts{
                match stmt{
                    Stmt::Local(stmt)=>{
                        vars.push(generate_shvar_defs(stmt));
                    }
                    Stmt::Item(stmt)=>{
                        match stmt{
                            Item::Struct(item)=>{
                                structs.push(generate_struct_def(item));
                            }
                            Item::Const(item)=>{
                                consts.push(generate_const_def(item));
                            }
                            Item::Fn(item)=>{
                                fns.push(generate_fn_def(item));
                            }
                            _=>{
                                return error(stmt.span(), "Unexpected statement")
                            }
                        }
                    }
                    Stmt::Expr(stmt)=>{
                            return error(stmt.span(), "Expression not expected here")
                    }
                    Stmt::Semi(stmt, _tok)=>{
                            return error(stmt.span(), "Statement not expected here")
                    }
                }
            }
        },
        _=>{
            return error(expr.span(), "Expecting block")
        }
    };
    quote!{ 
        ShAst{
            types:Vec::new(),//{let mut v=Vec::new();#(v.push(#types);)*v},
            vars:{let mut v=Vec::new();#(v.push(#vars);)*v},
            consts:{let mut v=Vec::new();#(v.push(#consts);)*v},
            fns:{let mut v=Vec::new();#(v.push(#fns);)*v} 
        }
    }

}

// The actual macro
#[proc_macro_hack]
pub fn shader_ast(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    
    let parsed = syn::parse_macro_input!(input as syn::Expr);

    let ts = generate_root(parsed);
    proc_macro::TokenStream::from(ts)
}
 