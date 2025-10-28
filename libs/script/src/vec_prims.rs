
use crate::vm::*;
use crate::value::*;
use crate::heap::*;
use crate::traits::*;
use crate::array::*;

impl<T> ScriptApply for Vec<T> where T: ScriptApply + ScriptNew + 'static + ScriptDeriveMarker{
    fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_apply(&mut self, vm:&mut ScriptVm, apply:&mut ApplyScope, value:ScriptValue){
        if let Some(obj) = value.as_object(){
            let len = vm.heap.vec_len(obj);
            self.resize_with(len, || ScriptNew::script_new(vm));
            for i in 0..len{
                if let Some(value) = vm.heap.vec_value_if_exist(obj, i){
                    self[i].script_apply(vm, apply, value);
                }
            }
        }
        else if value.is_nil(){
            self.clear()
        }
    }
    fn script_to_value(&self, vm:&mut ScriptVm)->ScriptValue{
        let obj = vm.heap.new_object();
        for v in self.iter(){
            let v = v.script_to_value(vm);
            vm.heap.vec_push(obj, NIL, v, &vm.thread.trap);
        }
        obj.into()
    } 
}

impl ScriptHook for Vec<u8> {}
impl ScriptNew for Vec<u8> {
    fn script_type_id_static()->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_type_check(heap:&ScriptHeap, value:ScriptValue)->bool{
        if let Some(obj) = value.as_object(){
            for kv in heap.vec_ref(obj){
                if !kv.value.is_number(){
                    return false
                }
            }
            return true
        }
        else if let Some(arr) = value.as_array(){
            match heap.array_ref(arr){
                ScriptArrayStorage::ScriptValue(vec)=>{
                    for v in vec{ 
                        if !v.is_number(){return false}
                    }
                    return true
                },
                ScriptArrayStorage::F32(_)=> return true,
                ScriptArrayStorage::U32(_)=> return true,
                ScriptArrayStorage::U16(_)=> return true,
                ScriptArrayStorage::U8(_)=> return true
            }
        }
        value.is_string_like() || value.is_nil()
    }
    fn script_default(vm:&mut ScriptVm)->ScriptValue{
        vm.heap.new_object().into()
    }
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()}
    fn script_proto_build(vm:&mut ScriptVm, _props:&mut ScriptTypeProps)->ScriptValue{
        vm.heap.new_object().into()
    }
}

impl ScriptApply for Vec<u8> {
    fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut ApplyScope, value:ScriptValue){
        if let Some(obj) = value.as_object(){
            self.clear();
            for kv in vm.heap.vec_ref(obj){
                self.push(kv.value.as_f64().unwrap_or(0.0) as _);
            }
        }
        if let Some(arr) = value.as_array(){
            self.clear();
            match vm.heap.array_ref(arr){
                ScriptArrayStorage::ScriptValue(vec)=> for v in vec{ self.push((*v).into()) }
                ScriptArrayStorage::F32(vec)=> for v in vec{ self.push(*v as _) }
                ScriptArrayStorage::U32(vec)=> for v in vec{ self.push(*v as _) }
                ScriptArrayStorage::U16(vec)=> for v in vec{ self.push(*v as _) }
                ScriptArrayStorage::U8(vec)=> for v in vec{ self.push(*v as _) }
            }
        }
        else if let Some(str) = value.as_string(){
            let str = vm.heap.string(str);
            self.clear();
            self.extend(str.as_bytes());
        }
        else if value.as_inline_string(|s|{
            self.clear();
            self.extend(s.as_bytes());
        }).is_some(){
        }
        else if value.is_nil(){
            self.clear();
        }
        else{
            vm.thread.trap.err_wrong_type_in_apply();
        }
    }
    fn script_to_value(&self, vm:&mut ScriptVm)->ScriptValue{
        let arr = vm.heap.new_array();
        let astore = vm.heap.array_mut(arr, &vm.thread.trap).unwrap();
        if let ScriptArrayStorage::U8(v) = astore{v.clear();v.extend(self)}
        else{*astore = ScriptArrayStorage::U8(self.clone());}
        arr.into()
    } 
}

impl ScriptHook for Vec<ScriptValue> {}
impl ScriptNew for Vec<ScriptValue> {
    fn script_type_id_static()->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{
        value.is_object() || value.is_array() || value.is_nil()
    }
    fn script_default(vm:&mut ScriptVm)->ScriptValue{
        vm.heap.new_object().into()
    }
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()}
    fn script_proto_build(vm:&mut ScriptVm, _props:&mut ScriptTypeProps)->ScriptValue{
        vm.heap.new_object().into()
    }
}
impl ScriptApply for Vec<ScriptValue> {
    fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut ApplyScope, value:ScriptValue){
        if let Some(obj) = value.as_object(){
            self.clear();
            for kv in vm.heap.vec_ref(obj){
                self.push(kv.value);
            }
        }
        if let Some(arr) = value.as_array(){
            self.clear();
            match vm.heap.array_ref(arr){
                ScriptArrayStorage::ScriptValue(vec)=> for v in vec{ self.push((*v).into()) }
                ScriptArrayStorage::F32(vec)=> for v in vec{ self.push((*v).into()) }
                ScriptArrayStorage::U32(vec)=> for v in vec{ self.push((*v).into()) }
                ScriptArrayStorage::U16(vec)=> for v in vec{ self.push((*v).into()) }
                ScriptArrayStorage::U8(vec)=> for v in vec{ self.push((*v).into()) }
            }
        }
        else if value.is_nil(){
            self.clear();
        }
        else{
            vm.thread.trap.err_wrong_type_in_apply();
        }
    }
    fn script_to_value(&self, vm:&mut ScriptVm)->ScriptValue{
        let arr = vm.heap.new_array();
        let astore = vm.heap.array_mut(arr, &vm.thread.trap).unwrap();
        if let ScriptArrayStorage::ScriptValue(v) = astore{v.clear();v.extend(self)}
        else{*astore = ScriptArrayStorage::ScriptValue(self.clone());}
        arr.into()
    } 
}