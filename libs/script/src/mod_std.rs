use crate::makepad_live_id::live_id::*;
use crate::heap::*;
use crate::value::*;
use crate::makepad_live_id_macros::*;
use crate::native::*;
use crate::*;

pub fn define_std_module(heap:&mut ScriptHeap, native:&mut ScriptNative){
    let std = heap.new_module(id!(std));
            
    native.add_method(heap, std, id!(assert), script_args!(v= NIL), |vm, args|{
        if let Some(x) = script_value!(vm, args.v).as_bool(){
            if x == true{
                return NIL
            }
        }
        script_err_assert_fail!(vm.thread.trap, "assertion failed")
    });
    
    //native.add_method(heap, std, id!(err), script_args!(), |vm, _args|{
     //   return vm.thread.last_err
    //});
            
    let range = heap.new_with_proto(id!(range).into());
    heap.set_value_def(std, id!(Range).into(), range.into());
            
    native.add_method(heap, range, id!(step), script_args!(x= 0.0), |vm, args|{
        if let Some(sself) = script_value!(vm, args.self).as_object(){
            if let Some(x) = script_value!(vm, args.x).as_f64(){
                set_script_value!(vm, sself.step = x);
            }
            return sself.into()
        }
        NIL
    });
    
    native.add_method(heap, std, id!(log), script_args_def!(what=NIL), |vm, args|{
        let what = script_value!(vm, args.what);
        vm.thread.log(vm.heap, vm.code, what);
        NIL
    });
    
    native.add_method(heap, std, id!(print), script_args_def!(what=NIL), |vm, args|{
        let what = script_value!(vm, args.what);
        if vm.heap.string_with(what, |_heap, str|{
            print!("{}", str);
        }).is_none(){
            vm.heap.temp_string_with(|heap, temp|{
                heap.cast_to_string(what, temp);
                print!("{}", what)
            });
        }
        NIL
    });
    
    native.add_method(heap, std, id!(println), script_args_def!(what=NIL), |vm, args|{
        let what = script_value!(vm, args.what);
        if vm.heap.string_with(what, |_heap, str|{
            println!("{}", str);
        }).is_none(){
            vm.heap.temp_string_with(|heap, temp|{
                heap.cast_to_string(what, temp);
                println!("{}", what)
            });
        }
        NIL
    });
    
    //native.add_method(heap, std, id!(to_metal_shader), script_args!(entry=NIL), |vm, _args|{
        
     //   return vm.thread.last_err
    //});
    
    native.add_method(heap, std, id!(set_type_default), script_args!(obj=NIL), |vm, args|{
        if let Some(obj) = script_value!(vm, args.obj).as_object(){
            if vm.heap.set_type_default(obj){
                return obj.into()
            }
        }
        NIL
    });
}
