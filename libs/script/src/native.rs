use crate::vm::*;
use crate::value::*;
use crate::makepad_live_id::live_id::*;
use crate::heap::*;
use crate::makepad_live_id::*;
use crate::object::*;
use crate::array::*;
use crate::string::*;
use crate::function::*;


#[macro_export]
macro_rules! script_value_f64{
    ($ctx:ident, $args:ident.$id: ident)=>{
        $ctx.heap.cast_to_f64($ctx.heap.value($args, id!($id).into(),&$ctx.thread.trap), $ctx.thread.trap.ip)
    };
    ($ctx:ident, $obj:ident[$index: expr])=>{
        $ctx.heap.cast_to_f64($ctx.heap.vec_value($obj, ($index) as usize), $ctx.thread.ip())
    }
}

#[macro_export]
macro_rules! script_value_bool{
    ($ctx:ident, $args:ident.$id: ident)=>{
        $ctx.heap.cast_to_bool($ctx.heap.value($args, id!($id).into(),NIL), $ctx.thread.ip())
    };
    ($ctx:ident, $obj:ident[$index: expr])=>{
        $ctx.heap.cast_to_bool($ctx.heap.vec_value($obj, ($index) as usize), $ctx.thread.ip())
    }
}
        
#[macro_export]
macro_rules! script_value{
    ($vm:ident, $obj:ident.$id: ident)=>{
        $vm.heap.value(($obj).into(), id!($id).into(),&$vm.thread.trap)
    };
    ($vm:ident, $obj:ident.$id:ident.$id2:ident)=>{
        $vm.heap.value($vm.heap.value(($obj).into(), id!($id).into(),&$vm.thread.trap).into(), id!($id2).into(),&$vm.thread.trap)
    };
    ($vm:ident, $obj:ident[$index: expr])=>{
        $vm.heap.vec_value(($obj).into(), ($index) as usize,&$vm.thread.trap)
    };
    ($vm:ident, $obj:ident as array[$index: expr])=>{
        $vm.heap.array_index(($obj).into(), ($index) as usize,&$vm.thread.trap)
    }
}

#[macro_export]
macro_rules! script_has_proto{
    ($vm:ident, $what:ident, $obj:ident.$id: ident)=>{
        {
           let proto = $vm.heap.value(($obj).into(), id!($id).into(),&$vm.thread.trap);
           $vm.heap.has_proto(($what).into(), proto)
        }
    };
}

#[macro_export]
macro_rules! script_is_fn{
    ($vm:ident, $what:ident, $obj:expr)=>{
        {
            $vm.heap.is_fn(($obj).into())
        }
    };
}

#[macro_export]
macro_rules! script_array_index{
    ($vm:ident, $obj:ident[$index: expr])=>{
        $vm.heap.array_index(($obj).into(), ($index) as usize,&$vm.thread.trap)
    }
}

#[macro_export]
macro_rules! set_script_value{
    ($vm:ident, $obj:ident.$id: ident=$value:expr)=>{
        $vm.heap.set_value($obj, id!($id).into(), ($value).into(), &$vm.thread.trap)
    };
    ($vm:ident, $obj:ident[$index: expr]=$value:expr)=>{
        $vm.heap.set_vec_value($obj, ($index) as usize, ($value).into(), &$vm.thread.trap)
    }
}

#[macro_export]
macro_rules! set_script_value_to_api{
    ($vm:ident, $obj:ident.$id: ident=$val:expr)=>{
        {
            let v = $val::script_api($vm);
            $vm.heap.set_value(($obj).into(), id_lut!($id).into(), v, &$vm.thread.trap);
        }
    };
    ($vm:ident, $obj:ident.$id: ident)=>{
        {
            let v = $id::script_api($vm);
            $vm.heap.set_value(($obj).into(), id_lut!($id).into(), v, &$vm.thread.trap);
        }
    };
}

#[macro_export]
macro_rules! set_script_value_to_pod{
    ($vm:ident, $obj:ident.$id: ident=$val:expr)=>{
        {
            let v = $val::script_pod($vm).expect("Cant make a pod type");
            // Set the Pod type name to the type name (e.g., DrawCallUniforms)
            $vm.heap.pod_type_name_set(v, id_lut!($id));
            $vm.heap.set_value(($obj).into(), id_lut!($id).into(), v.into(), &$vm.thread.trap);
        }
    };
    ($vm:ident, $obj:ident.$id: ident)=>{
        {
            let v = $id::script_pod($vm).expect("Cant make a pod type");
            // Set the Pod type name to the type name (e.g., DrawCallUniforms)
            $vm.heap.pod_type_name_set(v, id_lut!($id));
            $vm.heap.set_value(($obj).into(), id_lut!($id).into(), v.into(), &$vm.thread.trap);
        }
    };
}


#[macro_export]
macro_rules! script_args{
    ($($id:ident=$val:expr),*)=>{
        &[$((id!($id), ($val).into()),)*]
    }
}

#[macro_export]
macro_rules! script_args_def{
    ($($id:ident=$val:expr),*)=>{
        &[$((id_lut!($id), ($val).into()),)*]
    }
}

pub type NativeGetterFn = Box<dyn Fn(&mut ScriptVm, ScriptValue, LiveId)->ScriptValue + 'static>;
pub type NativeSetterFn = Box<dyn Fn(&mut ScriptVm, ScriptValue, LiveId, ScriptValue)->ScriptValue + 'static>;
pub type NativeFn = Box<dyn Fn(&mut ScriptVm, ScriptObject)->ScriptValue + 'static>;

#[derive(Default)]
pub struct ScriptNative{
    pub(crate) functions: Vec<NativeFn>,
    pub(crate) type_table: Vec<LiveIdMap<LiveId, ScriptObject>>,
    pub(crate) handle_type: LiveIdMap<LiveId,ScriptHandleType>,
    pub(crate) getters: Vec<NativeGetterFn>,
    pub(crate) setters: Vec<NativeSetterFn>,
}

impl ScriptNative{
    pub fn new(h:&mut ScriptHeap)->Self{
        let mut native = Self::default();
        native.add_shared(h);
        ScriptObjectData::add_type_methods(&mut native, h);
        ScriptArrayData::add_type_methods(&mut native, h);
        ScriptStringData::add_type_methods(&mut native, h);
        native
    }
    
    pub fn add_fn<F>(&mut self, heap:&mut ScriptHeap, args:&[(LiveId,ScriptValue)], f: F)-> ScriptObject
    where F: Fn(&mut ScriptVm, ScriptObject)->ScriptValue + 'static{
        let fn_index = self.functions.len();
        let fn_obj = heap.new_with_proto(id!(native).into());
        heap.set_object_storage_vec2(fn_obj);
        heap.set_fn(fn_obj, ScriptFnPtr::Native(NativeId{index: fn_index as u32}));

        for (arg, def) in args{
            heap.set_value_def(fn_obj, (*arg).into(), *def);
        }
        
        self.functions.push(Box::new(f));
        
        fn_obj
    }
    
    pub fn add_method<F>(&mut self, heap:&mut ScriptHeap, module:ScriptObject, method:LiveId, args:&[(LiveId, ScriptValue)], f: F) 
    where F: Fn(&mut ScriptVm, ScriptObject)->ScriptValue + 'static{
        // lets get the 
        let fn_obj = self.add_fn(heap, args, f);
        heap.set_value_def(module, method.into(), fn_obj.into());
    }
    
    pub fn new_handle_type(&mut self, heap:&mut ScriptHeap, id:LiveId)->ScriptHandleType{
        let ht = self.type_table.len() - ScriptValueType::REDUX_HANDLE_FIRST.to_index();
        if ht >= ScriptValueType::REDUX_HANDLE_MAX as usize{
            panic!("Too many handle types (max {})", ScriptValueType::REDUX_HANDLE_MAX);
        }
        let ty = ScriptHandleType(ht as u8);
        self.handle_type.insert(id, ty);
        self.add_type_method(heap, ty.to_redux(), id!(ty), &[], move |_, _|{id.escape()});
        ty
    }
            
    pub fn set_type_getter<F>(&mut self, ty_redux:ScriptTypeRedux,f: F) 
    where F: Fn(&mut ScriptVm, ScriptValue, LiveId)->ScriptValue + 'static{
        self.getters[ty_redux.to_index()] = Box::new(f)
    }
            
    pub fn set_type_setter<F>(&mut self, ty_redux:ScriptTypeRedux,f: F) 
    where F: Fn(&mut ScriptVm, ScriptValue, LiveId, ScriptValue)->ScriptValue + 'static{
        self.setters[ty_redux.to_index()] = Box::new(f)
    }
            
    pub fn add_type_method<F>(&mut self, heap:&mut ScriptHeap,ty_redux:ScriptTypeRedux, method:LiveId,  args:&[(LiveId,ScriptValue)], f: F) 
    where F: Fn(&mut ScriptVm, ScriptObject)->ScriptValue + 'static{
        let fn_obj = self.add_fn(heap, args, f);
        if ty_redux.to_index() as usize >= self.type_table.len(){
            self.type_table.resize_with( ty_redux.to_index() + 1, || Default::default());
            self.getters.resize_with( ty_redux.to_index() + 1, || Box::new(|vm, _, _|{vm.thread.trap.err_invalid_prop_name()}));
            self.setters.resize_with( ty_redux.to_index() + 1, || Box::new(|vm, _, _, _|{vm.thread.trap.err_invalid_prop_name()}));
        }
        self.type_table[ ty_redux.to_index()].insert(method,fn_obj);
    }
            
    pub fn add_shared(&mut self, heap:&mut ScriptHeap){
        self.add_type_method(heap, ScriptValueType::REDUX_NUMBER, id!(ty), &[], |_, _|{id!(number).escape()});
        self.add_type_method(heap, ScriptValueType::REDUX_NAN, id!(ty), &[], |_, _|{id!(nan).escape()});
        self.add_type_method(heap, ScriptValueType::REDUX_BOOL, id!(ty), &[], |_, _|{id!(bool).escape()});
        self.add_type_method(heap, ScriptValueType::REDUX_NIL, id!(ty), &[], |_, _|{id!(nil).escape()});
        self.add_type_method(heap, ScriptValueType::REDUX_COLOR, id!(ty), &[], |_, _|{id!(color).escape()});
        self.add_type_method(heap, ScriptValueType::REDUX_STRING, id!(ty), &[], |_, _|{id!(string).escape()});
        self.add_type_method(heap, ScriptValueType::REDUX_OBJECT, id!(ty), &[], |_, _|{id!(object).escape()});
        self.add_type_method(heap, ScriptValueType::REDUX_ARRAY, id!(ty), &[], |_, _|{id!(rsid).escape()});
        self.add_type_method(heap, ScriptValueType::REDUX_OPCODE, id!(ty), &[], |_, _|{id!(opcode).escape()});
        self.add_type_method(heap, ScriptValueType::REDUX_ERR, id!(ty), &[], |_, _|{id!(err).escape()});
        self.add_type_method(heap, ScriptValueType::REDUX_ID, id!(ty), &[], |_, _|{id!(id).escape()});
                                
        let types = [
            (ScriptValueType::REDUX_NUMBER, id!(is_number)),
            (ScriptValueType::REDUX_NAN, id!(is_nan)),
            (ScriptValueType::REDUX_BOOL, id!(is_bool)),
            (ScriptValueType::REDUX_NIL, id!(is_nil)),
            (ScriptValueType::REDUX_COLOR, id!(is_color)),
            (ScriptValueType::REDUX_STRING, id!(is_string)),
            (ScriptValueType::REDUX_OBJECT, id!(is_object)),
            (ScriptValueType::REDUX_ARRAY, id!(is_array)),
            (ScriptValueType::REDUX_OPCODE, id!(is_opcode)),
            (ScriptValueType::REDUX_ERR, id!(is_err)),
            (ScriptValueType::REDUX_ID, id!(is_id))
        ];
                        
        for (ty,_) in types {
            self.add_type_method(heap, ty, id!(to_json), &[], |vm, args|{
                let sself = script_value!(vm, args.self);vm.heap.to_json(sself)
            });
            self.add_type_method(heap, ty, id!(to_number), &[], |vm, args|{
                let sself = script_value!(vm, args.self);
                vm.heap.cast_to_f64(sself, vm.thread.trap.ip).into()
            });
            if ty != ScriptValueType::REDUX_ARRAY{
                self.add_type_method(heap, ty, id!(to_string), &[], |vm, args|{
                    let sself = script_value!(vm, args.self);
                    if sself.is_string_like(){
                        return sself
                    }
                    vm.heap.new_string_with(|heap, out|{
                        heap.cast_to_string(sself, out);
                    })
                });
            }
        };
                        
        for (ty,id) in types{
            self.add_type_method(heap, ScriptValueType::REDUX_NUMBER, id, &[], move |_, _|{ (ty == ScriptValueType::REDUX_NUMBER).into()});
            self.add_type_method(heap,ScriptValueType::REDUX_NAN, id, &[], move |_, _|{ (ty == ScriptValueType::REDUX_NAN).into()});
            self.add_type_method(heap, ScriptValueType::REDUX_BOOL, id, &[], move |_, _|{ (ty == ScriptValueType::REDUX_BOOL).into()});
            self.add_type_method(heap, ScriptValueType::REDUX_NIL, id, &[], move |_, _|{ (ty == ScriptValueType::REDUX_NIL).into()});
            self.add_type_method(heap, ScriptValueType::REDUX_COLOR, id, &[], move |_, _|{ (ty == ScriptValueType::REDUX_COLOR).into()});
            self.add_type_method(heap, ScriptValueType::REDUX_STRING, id, &[], move |_, _|{ (ty == ScriptValueType::REDUX_STRING).into()});
            self.add_type_method(heap, ScriptValueType::REDUX_OBJECT, id, &[], move |_, _|{ (ty == ScriptValueType::REDUX_OBJECT).into()});
            self.add_type_method(heap, ScriptValueType::REDUX_ARRAY, id, &[], move |_, _|{ (ty == ScriptValueType::REDUX_ARRAY).into()});
            self.add_type_method(heap, ScriptValueType::REDUX_OPCODE, id, &[], move |_, _|{ (ty == ScriptValueType::REDUX_OPCODE).into()});
            self.add_type_method(heap, ScriptValueType::REDUX_ERR, id, &[], move |_, _|{ (ty == ScriptValueType::REDUX_ERR).into()});
            self.add_type_method(heap, ScriptValueType::REDUX_ID, id, &[], move |_, _|{ (ty == ScriptValueType::REDUX_ID).into()});
        }
    }
    
}
 
