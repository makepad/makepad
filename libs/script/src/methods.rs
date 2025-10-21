use crate::makepad_id::*;
use crate::heap::*;
use crate::value::*;
use crate::native::*;
use crate::vm::*;
use crate::*;

#[derive(Default)]
pub struct ScriptTypeMethods{
    pub type_table: Vec<IdMap<Id, Object>>,
}

impl ScriptTypeMethods{
    pub fn new(h:&mut ScriptHeap, native:&mut ScriptNative)->Self{
        let mut t = Self::default();
        t.add_shared(h, native);
        t.add_object(h, native);
        t
    }
    
    pub fn add<F>(&mut self, heap:&mut ScriptHeap, native:&mut ScriptNative, args:&[(Id,Value)], ty_redux:usize, method:Id, f: F) 
    where F: Fn(&mut Vm, Object)->Value + 'static{
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
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(proto), |vm, args|{
            if let Some(this) = value!(vm, args.this).as_object(){
                return vm.heap.proto(this)
            }
            vm.thread.trap.err_unexpected()
        });
        
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(push), |vm, args|{
            if let Some(this) = value!(vm, args.this).as_object(){
                return vm.heap.vec_push_vec(this, args, &mut vm.thread.trap);
            }
            vm.thread.trap.err_unexpected()
        });
        
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(pop), |vm, args|{
            if let Some(this) = value!(vm, args.this).as_object(){
                return vm.heap.vec_pop(this, &mut vm.thread.trap)
            }
            vm.thread.trap.err_unexpected()
        });
        
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(len), |vm, args|{
            if let Some(this) = value!(vm, args.this).as_object(){
                return vm.heap.vec_len(this).into()
            }
            vm.thread.trap.err_unexpected()
        });
            
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(extend), |vm, args|{
            if let Some(this) = value!(vm, args.this).as_object(){
                return vm.heap.vec_push_vec_of_vec(this, args, false, &mut vm.thread.trap);
            }
            vm.thread.trap.err_unexpected()
        });
            
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(import), |vm, args|{
            if let Some(this) = value!(vm, args.this).as_object(){
                return vm.heap.vec_push_vec_of_vec(this, args, true, &mut vm.thread.trap);
            }
            vm.thread.trap.err_unexpected()
        });
        
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(freeze), |vm, args|{
            if let Some(this) = value!(vm, args.this).as_object(){
                vm.heap.freeze(this);
                return this.into()
            }
            vm.thread.trap.err_unexpected()
        });
        
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(freeze_api), |vm, args|{
            if let Some(this) = value!(vm, args.this).as_object(){
                vm.heap.freeze_api(this);
                return this.into()
            }
            vm.thread.trap.err_unexpected()
        });
        
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(freeze_module), |vm, args|{
            if let Some(this) = value!(vm, args.this).as_object(){
                vm.heap.freeze_module(this);
                return this.into()
            }
            vm.thread.trap.err_unexpected()
        });
        
        self.add(h, native, &[], ValueType::REDUX_OBJECT, id!(freeze_component), |vm, args|{
            if let Some(this) = value!(vm, args.this).as_object(){
                vm.heap.freeze_component(this);
                return this.into()
            }
            vm.thread.trap.err_unexpected()
        });
        
        self.add(h, native, args!(cb=NIL), ValueType::REDUX_OBJECT, id!(retain), |vm, args|{
            if let Some(this) = value!(vm, args.this).as_object(){
                let fnptr = value!(vm, args.cb);
                let mut i = 0;
                while i < vm.heap.vec_len(this){
                    let value = value!(vm, this[i]);
                    let ret = vm.call(fnptr, &[value]);
                    if ret.is_err(){
                        return ret;
                    }
                    if !vm.heap.cast_to_bool(ret){
                        vm.heap.vec_remove(this, i, &mut vm.thread.trap);
                    }
                    else{
                        i += 1
                    }
                }
                return NIL
            }
            vm.thread.trap.err_notimpl()
        });
    }     
}    
      
