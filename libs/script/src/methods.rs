use crate::makepad_value::id::*;
use crate::heap::*;
use crate::makepad_value::value::*;
use crate::makepad_value_derive::*;
use crate::native::*;
use crate::script::*;
use crate::*;

#[derive(Default)]
pub struct ScriptTypeMethods{
    pub type_table: Vec<IdMap<Id, ObjectPtr>>,
}

impl ScriptTypeMethods{
    pub fn new(h:&mut ScriptHeap, native:&mut ScriptNative)->Self{
        let mut t = Self::default();
        t.add_shared(h, native);
        t.add_object(h, native);
        t
    }
    
    pub fn add<F>(&mut self, heap:&mut ScriptHeap, native:&mut ScriptNative, args:&[(Id,Value)], ty_redux:usize, method:Id, f: F) 
    where F: Fn(&mut ScriptCtx, ObjectPtr)->Value + 'static{
        let fn_obj = native.add(heap, args, f);
                
        if ty_redux >= self.type_table.len(){
            self.type_table.resize_with(ty_redux + 1, || Default::default());
        }
        self.type_table[ty_redux].insert(method,fn_obj);
    }
    
    pub fn add_shared(&mut self, h:&mut ScriptHeap, native:&mut ScriptNative){
        self.add(h, native, &[], ValueType::REDUX_NUMBER, id!(ty), |_, _|{id!(number).escape()});
        self.add(h, native, &[], ValueType::REDUX_NAN, id!(ty), |_, _|{id!(nan).escape()});
        self.add(h, native, &[], ValueType::REDUX_BOOL, id!(ty), |_, _|{id!(bool).escape()});
        self.add(h, native, &[], ValueType::REDUX_NIL, id!(ty), |_, _|{id!(nil).escape()});
        self.add(h, native, &[], ValueType::REDUX_COLOR, id!(ty), |_, _|{id!(color).escape()});
        self.add(h, native, &[], ValueType::REDUX_STRING, id!(ty), |_, _|{id!(string).escape()});
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(ty), |_, _|{id!(object).escape()});
        self.add(h, native, &[], ValueType::REDUX_RSID, id!(ty), |_, _|{id!(rsid).escape()});
        self.add(h, native, &[], ValueType::REDUX_OPCODE, id!(ty), |_, _|{id!(opcode).escape()});
        self.add(h, native, &[], ValueType::REDUX_ERR, id!(ty), |_, _|{id!(err).escape()});
        self.add(h, native, &[], ValueType::REDUX_ID, id!(ty), |_, _|{id!(id).escape()});
        for (ty,id) in [
            (ValueType::REDUX_NUMBER, id!(is_number)),
            (ValueType::REDUX_NAN, id!(is_nan)),
            (ValueType::REDUX_BOOL, id!(is_bool)),
            (ValueType::REDUX_NIL, id!(is_nil)),
            (ValueType::REDUX_COLOR, id!(is_color)),
            (ValueType::REDUX_STRING, id!(is_string)),
            (ValueType::REDUX_OBJECT, id!(is_object)),
            (ValueType::REDUX_RSID, id!(is_rsid)),
            (ValueType::REDUX_OPCODE, id!(is_opcode)),
            (ValueType::REDUX_ERR, id!(is_err)),
            (ValueType::REDUX_ID, id!(is_id))
        ]{
            self.add(h, native, &[], ValueType::REDUX_NUMBER, id, move |_, _|{ (ty == ValueType::REDUX_NUMBER).into()});
            self.add(h, native, &[], ValueType::REDUX_NAN, id, move |_, _|{ (ty == ValueType::REDUX_NAN).into()});
            self.add(h, native, &[], ValueType::REDUX_BOOL, id, move |_, _|{ (ty == ValueType::REDUX_BOOL).into()});
            self.add(h, native, &[], ValueType::REDUX_NIL, id, move |_, _|{ (ty == ValueType::REDUX_NIL).into()});
            self.add(h, native, &[], ValueType::REDUX_COLOR, id, move |_, _|{ (ty == ValueType::REDUX_COLOR).into()});
            self.add(h, native, &[], ValueType::REDUX_STRING, id, move |_, _|{ (ty == ValueType::REDUX_STRING).into()});
            self.add(h, native, &[], ValueType::REDUX_OBJECT, id, move |_, _|{ (ty == ValueType::REDUX_OBJECT).into()});
            self.add(h, native, &[], ValueType::REDUX_RSID, id, move |_, _|{ (ty == ValueType::REDUX_RSID).into()});
            self.add(h, native, &[], ValueType::REDUX_OPCODE, id, move |_, _|{ (ty == ValueType::REDUX_OPCODE).into()});
            self.add(h, native, &[], ValueType::REDUX_ERR, id, move |_, _|{ (ty == ValueType::REDUX_ERR).into()});
            self.add(h, native, &[], ValueType::REDUX_ID, id, move |_, _|{ (ty == ValueType::REDUX_ID).into()});
        }
    }
    
    pub fn add_object(&mut self, h: &mut ScriptHeap, native:&mut ScriptNative){
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(proto), |ctx, args|{
            if let Some(this) = value!(ctx, args.this).as_object(){
                return ctx.heap.object_proto(this)
            }
            Value::err_internal(ctx.thread.ip)
        });
        
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(push), |ctx, args|{
            if let Some(this) = value!(ctx, args.this).as_object(){
                ctx.heap.push_vec_into_vec(this, args);
                return NIL
            }
            Value::err_internal(ctx.thread.ip)
        });
        
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(pop), |ctx, args|{
            if let Some(this) = value!(ctx, args.this).as_object(){
                return ctx.heap.pop_vec(this)
            }
            Value::err_internal(ctx.thread.ip)
        });
        
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(len), |ctx, args|{
            if let Some(this) = value!(ctx, args.this).as_object(){
                return ctx.heap.vec_len(this).into()
            }
            Value::err_internal(ctx.thread.ip)
        });
            
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(extend), |ctx, args|{
            if let Some(this) = value!(ctx, args.this).as_object(){
                ctx.heap.push_vec_of_vec_into_vec(this, args, false);
                return NIL
            }
            Value::err_internal(ctx.thread.ip)
        });
            
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(import), |ctx, args|{
            if let Some(this) = value!(ctx, args.this).as_object(){
                ctx.heap.push_vec_of_vec_into_vec(this, args, true);
                return NIL
            }
            Value::err_internal(ctx.thread.ip)
        });
        
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(retain), |ctx, args|{
            if let Some(this) = value!(ctx, args.this).as_object(){
                let fnptr = ctx.heap.vec_value(args, 0);
                let mut i = 0;
                while i < ctx.heap.vec_len(this){
                    let value = ctx.heap.vec_value(this, i);
                    let ret = ctx.thread.call(ctx.heap, ctx.code,  ctx.host, fnptr, &[value]);
                    if ret.is_err(){
                        return ret;
                    }
                    if !ctx.heap.cast_to_bool(ret){
                        ctx.heap.vec_remove(this, i);
                    }
                    else{
                        i += 1
                    }
                }
                return NIL
            }
            Value::err_notimpl(ctx.thread.ip)
        });
    }     
}    
      
