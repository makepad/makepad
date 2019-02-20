// Shared shader-compiler code for generating GLSL and Metal shading language

use crate::shader::*;
//use crate::cxshaders::*;

#[derive(Clone)]
pub struct Sl{
    pub sl:String,
    pub ty:String
}

#[derive(Clone)]
pub struct SlErr{
    pub msg:String
}

pub struct SlDecl{
    pub name:String,
    pub ty:String
}

#[derive(Clone)]
pub enum SlTarget{
    Pixel,
    Vertex
}

pub struct SlCx<'a>{
    pub depth:usize,
    pub target:SlTarget,
    pub defargs_fn:String,
    pub defargs_call:String,
    pub call_prefix:String,
    pub shader:&'a Shader,
    pub scope:Vec<SlDecl>,
    pub fn_deps:Vec<String>,
    pub fn_done:Vec<Sl>,
    pub auto_vary:Vec<ShVar>
}

pub enum MapCallResult{
    Rename(String),
    Rewrite(String, String),
    None
}

impl<'a> SlCx<'a>{
    pub fn scan_scope(&self, name:&str)->Option<&str>{
        if let Some(decl) = self.scope.iter().find(|i| i.name == name){
            return Some(&decl.ty);
        }
        None
    }
    pub fn get_type(&self, name:&str)->Result<&ShType, SlErr>{
        if let Some(ty) = self.shader.find_type(name){
            return Ok(ty);
        }
        Err(SlErr{msg:format!("Cannot find type {}", name)})
    }
}

impl ShExpr{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        match self{
            ShExpr::ShId(x)=>x.sl(cx),
            ShExpr::ShLit(x)=>x.sl(cx),
            ShExpr::ShAssign(x)=>x.sl(cx),
            ShExpr::ShCall(x)=>x.sl(cx),
            ShExpr::ShBinary(x)=>x.sl(cx),
            ShExpr::ShUnary(x)=>x.sl(cx),
            ShExpr::ShAssignOp(x)=>x.sl(cx),
            ShExpr::ShIf(x)=>x.sl(cx),
            ShExpr::ShWhile(x)=>x.sl(cx),
            ShExpr::ShForLoop(x)=>x.sl(cx),
            ShExpr::ShBlock(x)=>x.sl(cx),
            ShExpr::ShField(x)=>x.sl(cx),
            ShExpr::ShIndex(x)=>x.sl(cx),
            ShExpr::ShParen(x)=>x.sl(cx),
            ShExpr::ShReturn(x)=>x.sl(cx),
            ShExpr::ShBreak(x)=>x.sl(cx),
            ShExpr::ShContinue(x)=>x.sl(cx),
        }
    }
}


impl ShId{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        // ok so. we have to find our id on
        if let Some(ty) = cx.scan_scope(&self.name){
            Ok(Sl{sl:self.name.to_string(), ty:ty.to_string()})
        }
        else if let Some(cnst) = cx.shader.find_const(&self.name){
            Ok(Sl{sl:self.name.to_string(), ty:cnst.ty.to_string()})
        } 
        else if let Some(var) = cx.shader.find_var(&self.name){
            Ok(Sl{sl:cx.map_var(var), ty:var.ty.to_string()})
        } 
        else{ // id not found.. lets give an error
            Err(SlErr{
                msg:format!("Id {} not resolved, is it declared?", self.name)
            })
        }
    }
}

impl ShLit{
    pub fn sl(&self, _cx:&mut SlCx)->Result<Sl,SlErr>{
        // we do a literal
        match self{
            ShLit::Int(val)=>{
                Ok(Sl{sl:format!("{}", val), ty:"int".to_string()})
            }
            ShLit::Str(val)=>{
                Ok(Sl{sl:format!("\"{}\"", val), ty:"string".to_string()})
            }
            ShLit::Float(val)=>{
                if val.ceil() == *val{
                    Ok(Sl{sl:format!("{}.0", val), ty:"float".to_string()})
                }
                else{
                    Ok(Sl{sl:format!("{}", val), ty:"float".to_string()})
                }
            }
            ShLit::Bool(val)=>{
                Ok(Sl{sl:format!("{}", val), ty:"bool".to_string()})
            }
        }
    }
}

impl ShField{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        let base = self.base.sl(cx)?;
        // we now have to figure out the type of member
        let shty = cx.get_type(&base.ty)?;
        // lets get our member 
        if let Some(field) = shty.fields.iter().find(|i| i.name == self.member){
            Ok(Sl{
                sl:format!("{}.{}", base.sl, self.member),
                ty:field.ty.to_string()
            })
        }
        else{
            let mut mode = 0;
            let slots = shty.slots;
            
            if shty.name != "float" && shty.name != "vec2" && shty.name != "vec3" && shty.name != "vec4"{
                return  Err(SlErr{
                    msg:format!("member {} not found {}", self.member, base.ty)
                })
            }
            if self.member.len() >4 {
                return  Err(SlErr{
                    msg:format!("member {} not found or a valid swizzle of {}", self.member, base.ty)
                })
            }
            for chr in self.member.chars(){
                if chr == 'x' || chr == 'y' || chr == 'z' || chr == 'w'{
                    if chr == 'y' && slots<2{ mode = 3;}
                    else if chr == 'z' && slots<3{ mode = 3;}
                    else if chr == 'w' && slots<4{ mode = 3;};
                    if mode == 0{ mode = 1;}
                    else if mode != 1{
                        return  Err(SlErr{
                            msg:format!("member {} not a valid swizzle of {}", self.member, base.ty)
                        })
                    }
                }
                else if chr == 'r' || chr == 'g' || chr == 'b' || chr == 'a'{
                    if chr == 'r' && slots<2{ mode = 3;}
                    else if chr == 'g' && slots<3{ mode = 3;}
                    else if chr == 'b' && slots<4{ mode = 3;};                    
                    if mode == 0{ mode = 2;}
                    else if mode != 2{
                        return  Err(SlErr{
                            msg:format!("member {} not a valid swizzle of {}", self.member, base.ty)
                        })
                    }
                }
            }

            match self.member.len(){
                1=>return Ok(Sl{
                    sl:format!("{}.{}", base.sl, self.member),
                    ty:"float".to_string()
                }),
                2=>return Ok(Sl{
                    sl:format!("{}.{}", base.sl, self.member),
                    ty:"vec2".to_string()
                }),
                3=>return Ok(Sl{
                    sl:format!("{}.{}", base.sl, self.member),
                    ty:"vec3".to_string()
                }),
                4=>return Ok(Sl{
                    sl:format!("{}.{}", base.sl, self.member),
                    ty:"vec4".to_string()
                }),
                _=>Err(SlErr{
                    msg:format!("member {} not cannot be found on type {}", self.member, base.ty)
                })
            }
        }
    }
}

impl ShIndex{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        let base = self.base.sl(cx)?;
        let index = self.index.sl(cx)?;
        // limit base type to vec2/3/4
        if base.ty != "vec2" && base.ty != "vec3" && base.ty != "vec4"{
             Err(SlErr{
                msg:format!("index on unsupported type {}", base.ty)
            })
        }
        else {
            Ok(Sl{
                sl:format!("{}[{}]", base.sl, index.sl),
                ty:"float".to_string()
            })
        }
    }
}

impl ShAssign{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        let left = self.left.sl(cx)?;
        let right = self.right.sl(cx)?;
        if left.ty != right.ty{
            Err(SlErr{
                msg:format!("Left type {} not the same as right {} in assign {}={}", left.ty, right.ty, left.sl, right.sl)
            })
        }
        else{
            Ok(Sl{
                sl:format!("{} = {}", left.sl, right.sl),
                ty:left.ty
            })
        }
    }
}

impl ShAssignOp{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        let left = self.left.sl(cx)?;
        let right = self.right.sl(cx)?;

        if left.ty != right.ty{
            Err(SlErr{
                msg:format!("Left type {} not the same as right {} in assign op {}{}{}", left.ty, self.op.to_string(), right.ty, left.sl, right.sl)
            })
        }
        else{
            Ok(Sl{
                sl:format!("{}{}{}", left.sl, self.op.to_string(), right.sl),
                ty:left.ty
            })
        }
    }
}

impl ShBinary{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        let left = self.left.sl(cx)?;
        let right = self.right.sl(cx)?;
        if left.ty != right.ty{
            if left.ty == "float" && (right.ty == "vec2" || right.ty == "vec3" || right.ty == "vec4"){
                Ok(Sl{
                    sl:format!("{}{}{}", left.sl, self.op.to_string(), right.sl),
                    ty:right.ty
                })
            }
            else if right.ty == "float" && (left.ty == "vec2" || left.ty == "vec3" || left.ty == "vec4"){
                Ok(Sl{
                    sl:format!("{}{}{}", left.sl, self.op.to_string(), right.sl),
                    ty:left.ty
                })
            }
            else{
                Err(SlErr{
                    msg:format!("Left type {} not the same as right {} in binary op {}{}{}", left.ty, right.ty, left.sl, self.op.to_string(), right.sl)
                })
            }
        }
        else{
            Ok(Sl{
                sl:format!("{}{}{}", left.sl, self.op.to_string(), right.sl),
                ty:left.ty
            })
        }
    }
}

impl ShUnary{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        let expr = self.expr.sl(cx)?;
        Ok(Sl{
            sl:format!("{}{}", self.op.to_string(), expr.sl),
            ty:expr.ty
        })
    }
}

impl ShParen{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        let expr = self.expr.sl(cx)?;
        Ok(Sl{
            sl:format!("({})", expr.sl),
            ty:expr.ty
        })
    }
}

impl ShBlock{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        let mut sl = String::new();
        sl.push_str("{\n");
        cx.depth += 1;
        for stmt in &self.stmts{
            for _i in 0..cx.depth{
                sl.push_str("  ");
            }
            match &**stmt{
                ShStmt::ShLet(stmt) => {
                    let out = stmt.sl(cx)?;
                    sl.push_str(&out.sl);
                },
                ShStmt::ShExpr(stmt) => {
                    let out = stmt.sl(cx)?;
                    sl.push_str(&out.sl);
                }
                ShStmt::ShSemi(stmt) => {
                    let out = stmt.sl(cx)?;
                    sl.push_str(&out.sl);
                }
            }
            sl.push_str(";\n");
        }
        cx.depth -= 1;
        sl.push_str("}");
        Ok(Sl{
            sl:sl,
            ty:"void".to_string()
        })
    }
}

impl ShCall{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        // we have a call, look up the call type on cx
        let mut out = String::new();
        if let Some(shfn) = cx.shader.find_fn(&self.call){
            let mut defargs_call = "".to_string();
            if let Some(_block) = &shfn.block{ // not internal, so its a dep
                if cx.fn_deps.iter().find(|i| **i == self.call).is_none(){
                    cx.fn_deps.push(self.call.clone());
                }
                defargs_call = cx.defargs_call.to_string();
                out.push_str(&cx.call_prefix);
            };
            

            // lets check our args and compose return type
            let mut gen_t = "".to_string();

            let mut args_gl = Vec::new();
            // loop over args and typecheck / fill in generics
            for arg in &self.args{
                let arg_gl = arg.sl(cx)?;
                args_gl.push(arg_gl);
            };

            let map_call= cx.map_call(&self.call, &args_gl);
            let ret_ty;
            
            if let MapCallResult::Rewrite(rewrite, rty) = map_call{
                out.push_str(&rewrite);
                ret_ty = rty;
            }
            else{
                if let MapCallResult::Rename(name) = map_call{
                    out.push_str(&name);
                }
                else{
                    out.push_str(&self.call);
                }
                out.push_str("(");
                
                // loop over args and typecheck / fill in generics
                for (i, arg_gl) in args_gl.iter().enumerate(){
                    //let arg_gl = args_gl[i];//.sl(cx)?;
                    let in_ty = arg_gl.ty.clone();
                    if i != 0{
                        out.push_str(", ");
                    }
                    out.push_str(&arg_gl.sl);
                    // lets check the type against our shfn
                    if i >= shfn.args.len(){
                        return Err(SlErr{
                            msg:format!("Too many function arguments for call {} got:{} can use:{}", self.call, i+1, shfn.args.len())
                        })
                    }
                    // lets check our arg type
                    let fnarg = &shfn.args[i];
                    // lets see if ty is "T" or "O" or "F" or "B"
                    if fnarg.ty == "T"{
                        // we already have a gen_t but its not the same
                        if gen_t != "" && gen_t != in_ty{
                            return Err(SlErr{
                                msg:format!("Function type T incorrectly redefined for call {} type was {} given {} for arg {}", self.call, gen_t, in_ty, i) 
                            })
                        }
                        gen_t = in_ty;
                    }
                    else if fnarg.ty == "F"{ // we have to be a float type
                        if in_ty != "float" && in_ty != "vec2" && in_ty != "vec3" && in_ty != "vec4"{
                            return Err(SlErr{
                                msg:format!("Function type F is not a float-ty type for call {} for arg {} type {}", self.call, i, in_ty) 
                            })
                        }
                    }
                    else if fnarg.ty == "B"{ // have to be a boolvec
                        if in_ty != "bool" && in_ty != "bvec2" && in_ty != "bvec3" && in_ty != "bvec4"{
                            return Err(SlErr{
                                msg:format!("Function arg is not a bool-ty type for call {} for arg {} type {}", self.call, i, in_ty) 
                            })
                        }
                        gen_t = in_ty;
                    }
                    else if fnarg.ty != in_ty{
                        return Err(SlErr{
                            msg:format!("Arg wrong type for call {} for arg {} expected type {} got type {}", self.call, i, fnarg.ty, in_ty)
                        })
                    }
                }
                // we have less args provided than the fn signature
                // check if they were optional
                if self.args.len() < shfn.args.len(){
                    for i in self.args.len()..shfn.args.len(){
                        let fnarg = &shfn.args[i];
                        if fnarg.ty != "O"{
                            return Err(SlErr{
                                msg:format!("Not enough args for call {} not enough args provided at {}, possible {}", self.call, i, shfn.args.len())
                            })
                        }
                    }
                };
                ret_ty = if shfn.ret == "T" || shfn.ret == "B"{
                    gen_t
                }
                else{
                    shfn.ret.clone()
                };
                if defargs_call.len() != 0{
                    if self.args.len() != 0{
                        out.push_str(", ");
                    }
                    out.push_str(&defargs_call);
                }
                out.push_str(")");
            }
            // check our arg types
            // if our return type is T,
            // use one of the args marked T as its type
            // make sure all args are the same type T
            Ok(Sl{
                sl:out,
                ty:ret_ty
            })
        }
        else{
            // its a constructor call
            if let Some(glty) = cx.shader.find_type(&self.call){
                out.push_str(&cx.map_type(&self.call));
                out.push_str("(");
                // TODO check args
                for (i, arg) in self.args.iter().enumerate(){
                    let arg_gl = arg.sl(cx)?;
                    if i != 0{
                        out.push_str(", ");
                    }
                    out.push_str(&arg_gl.sl);
                }
                out.push_str(")");
                Ok(Sl{
                    sl:out,
                    ty:glty.name.clone()
                })
            }
            else{
                Err(SlErr{
                    msg:format!("Cannot find function {}", self.call)
                })
            }
        }
        
    }
}

impl ShIf{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        let mut out = "".to_string();
        out.push_str("if(");
        let cond = self.cond.sl(cx)?;
        out.push_str(&cond.sl);
        out.push_str(")");

        let then = self.then_branch.sl(cx)?;
        
        out.push_str(&then.sl);
        if let Some(else_branch) = &self.else_branch{
            let else_gl = else_branch.sl(cx)?;
            out.push_str("else ");
            out.push_str(&else_gl.sl);
        }
        
        Ok(Sl{
            sl:out,
            ty:"void".to_string()
        })
    }
}

impl ShWhile{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        let mut out = "".to_string();
        out.push_str("while(");
        let cond = self.cond.sl(cx)?;
        out.push_str(&cond.sl);
        out.push_str(")");

        let body = self.body.sl(cx)?;
        
        out.push_str(&body.sl);
        
        Ok(Sl{
            sl:out,
            ty:"void".to_string()
        })
    }
}

impl ShForLoop{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        let mut out = "".to_string();

        out.push_str("for(int ");
        out.push_str(&self.iter);
        out.push_str("=");
        
        let from = self.from.sl(cx)?;
        out.push_str(&from.sl);

        out.push_str(";");
        out.push_str(&self.iter);
        out.push_str(" < ");

        let to = self.to.sl(cx)?;
        out.push_str(&to.sl);

        out.push_str(";");
        out.push_str(&self.iter);
        out.push_str("++)");

        let body = self.body.sl(cx)?;

        out.push_str(&body.sl);
                
        Ok(Sl{
            sl:out,
            ty:"void".to_string()
        })
    }
}

impl ShReturn{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        let mut out = "".to_string();
        if let Some(expr) = &self.expr{
            let expr_gl = expr.sl(cx)?;
            out.push_str("return ");
            out.push_str(&expr_gl.sl);
        }
        else{
            out.push_str("return;");
        }
        Ok(Sl{
            sl:out,
            ty:"void".to_string()
        })
    }
}

impl ShBreak{
    pub fn sl(&self, _cx:&mut SlCx)->Result<Sl,SlErr>{
        Ok(Sl{
            sl:"break".to_string(),
            ty:"void".to_string()
        })
    }
}

impl ShContinue{
    pub fn sl(&self, _cx:&mut SlCx)->Result<Sl,SlErr>{
        Ok(Sl{
            sl:"continue".to_string(),
            ty:"void".to_string()
        })
    }
}

impl ShLet{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        let mut out = "".to_string();
        let init = self.init.sl(cx)?;

        let ty = init.ty.clone();
        if self.ty != "" && self.ty != init.ty{
            return Err(SlErr{
                msg:format!("Let definition {} type {} is different from initializer {}", self.name, self.ty, init.ty)
            })
        }

        out.push_str(&cx.map_type(&ty));
        out.push_str(" ");
        out.push_str(&self.name);
        out.push_str(" = ");
        
        // lets define our identifier on scope
        cx.scope.push(SlDecl{
            name:self.name.clone(),
            ty:init.ty.clone()
        });

        out.push_str(&init.sl);
        Ok(Sl{
            sl:out,
            ty:"void".to_string()
        })
    }
}

impl ShFn{
    pub fn sl(&self, cx:&mut SlCx)->Result<Sl,SlErr>{
        let mut out = "".to_string();
        out.push_str(&cx.map_type(&self.ret));
        out.push_str(" ");
        out.push_str(&cx.call_prefix);
        out.push_str(&self.name);
        out.push_str("(");
        for (i, arg) in self.args.iter().enumerate(){
            if i != 0{
                out.push_str(", ");
            }
            out.push_str(&cx.map_type(&arg.ty));
            out.push_str(" ");
            out.push_str(&arg.name);
            cx.scope.push(SlDecl{
                name:arg.name.clone(),
                ty:arg.ty.clone()
            });
        };
        if cx.defargs_fn.len() != 0{
            if self.args.len() != 0{
                out.push_str(", ");
            }
            out.push_str(&cx.defargs_fn);
        }
        out.push_str(")");
        if let Some(block) = &self.block{
            let block = block.sl(cx)?;
            out.push_str(&block.sl);
        };
        Ok(Sl{
            sl:out,
            ty:self.name.clone()
        })
    }
}

pub fn assemble_fn_and_deps(sh:&Shader, cx:&mut SlCx)->Result<String, SlErr>{

    let mut fn_local = Vec::new();
    loop{

        // find what deps we haven't done yet
        let fn_not_done = cx.fn_deps.iter().find(|cxfn|{
            if let Some(_done) = cx.fn_done.iter().find(|i| i.ty == **cxfn){
                false
            }
            else{
                true
            }
        });
        // do that dep.
        if let Some(fn_not_done) = fn_not_done{
            let fn_to_do = sh.find_fn(fn_not_done);
            if let Some(fn_to_do) = fn_to_do{
                cx.scope.clear();
                let result = fn_to_do.sl(cx)?;
                cx.fn_done.push(result.clone());
                fn_local.push(result);
            }
            else{
                return Err(SlErr{msg:format!("Cannot find entry function {}", fn_not_done)})
            }
        }
        else{
            break;
        }
    }
    // ok lets reverse concatinate it
    let mut out = String::new();
    for fnd in fn_local.iter().rev(){
        out.push_str(&fnd.sl);
        out.push_str("\n");
    }

    Ok(out)
}